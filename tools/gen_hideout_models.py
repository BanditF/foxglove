"""Generates the Python models of the Hideout contract from schema/hideout-v1.json (stdlib only).

    python tools/gen_hideout_models.py           # rewrite sdk/python/hideout_module/models.py
    python tools/gen_hideout_models.py --check   # exit 1 when the checked-in file is out of date

The schema is itself generated from the Rust types (crates/hideout-contract, `schema_is_current`),
so the chain is Rust types -> schema -> Python models, and each step has a drift check.
Each object becomes a dataclass with `from_json`/`to_json`; each string enum a `Literal` alias; a
`oneOf` tagged by a `const` property one class per variant and a `Union`.
"""

import json
import keyword
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "schema" / "hideout-v1.json"
OUT = ROOT / "sdk" / "python" / "hideout_module" / "models.py"


def pascal(text):
    return "".join(p[:1].upper() + p[1:] for p in re.split(r"[^A-Za-z0-9]+", text) if p)


def ident(name):
    name = re.sub(r"\W", "_", name)
    return name + "_" if keyword.iskeyword(name) else name


class Gen:
    def __init__(self, schema):
        self.schema = schema
        self.defs = {}  # name -> (schema, owner)
        self.out = []  # emitted blocks, in order
        self.done = set()

    def def_name(self, name, node, owner):
        seen = self.defs.get(name)
        if seen is None or seen[0] == node:
            self.defs[name] = (node, owner)
            return name
        return self.def_name(owner + name, node, owner)

    def run(self):
        top = {}
        for key, node in sorted(self.schema["types"].items()):
            owner = pascal(key.split(".", 1)[1])
            local = {}
            for name, d in sorted(node.get("$defs", {}).items()):
                local[name] = self.def_name(name, d, owner)
            body = {k: v for k, v in node.items() if k != "$defs"}
            top[key] = (owner, body, local)
        for key, (owner, body, local) in top.items():
            self.local, self.owner = local, owner
            for src, name in local.items():
                self.emit(name, self.defs[name][0])
            self.emit(owner, body)
        self.top = {key: owner for key, (owner, _, _) in top.items()}

    def spec(self, node, hint):
        """(spec expression, annotation) for a schema node."""
        if isinstance(node, bool):
            return "'any'", "Any"
        if "$ref" in node:
            ref = node["$ref"]
            name = self.owner if ref == "#" else self.local[ref.rsplit("/", 1)[1]]
            return f"('ref', {name!r})", name
        if "const" in node:
            return f"('const', {node['const']!r})", f"Literal[{node['const']!r}]"
        if "enum" in node:
            values = tuple(node["enum"])
            return f"('enum', {values!r})", "Literal[" + ", ".join(repr(v) for v in values) + "]"
        if "oneOf" in node:
            return self.union(node, hint)
        kind = node.get("type")
        if isinstance(kind, list):
            rest = [k for k in kind if k != "null"]
            inner = dict(node, type=rest[0] if len(rest) == 1 else rest)
            s, a = self.spec(inner, hint)
            return (f"('opt', {s})", f"Optional[{a}]") if "null" in kind else (s, a)
        if kind == "string":
            return "'str'", "str"
        if kind == "integer":
            return "'int'", "int"
        if kind == "number":
            return "'num'", "float"
        if kind == "boolean":
            return "'bool'", "bool"
        if kind == "array":
            s, a = self.spec(node.get("items", {}), hint + "Item")
            return f"('list', {s})", f"list[{a}]"
        if kind == "object" and "properties" in node:
            self.emit(hint, node)
            return f"('ref', {hint!r})", f"'{hint}'"
        if kind == "object":
            extra = node.get("additionalProperties", True)
            s, a = self.spec(extra, hint + "Value") if isinstance(extra, dict) else ("'any'", "Any")
            return f"('map', {s})", f"dict[str, {a}]"
        return "'any'", "Any"

    def union(self, node, name):
        alts = node["oneOf"]
        if all("const" in a or len(a.get("enum", ())) == 1 for a in alts):
            values = tuple(a["const"] if "const" in a else a["enum"][0] for a in alts)
            return self.spec({"enum": list(values)}, name)
        tags = [
            [k for k, p in a.get("properties", {}).items() if "const" in p] for a in alts
        ]
        common = set(tags[0]).intersection(*tags[1:]) if tags and tags[0] else set()
        if not common:
            return "'any'", "Any"
        tag = sorted(common)[0]
        variants = {}
        for a in alts:
            value = a["properties"][tag]["const"]
            cls = name + pascal(str(value))
            self.emit(cls, a)
            variants[value] = cls
        return f"('union', {tag!r}, {variants!r})", "Union[" + ", ".join(f"'{c}'" for c in variants.values()) + "]"

    def emit(self, name, node):
        if name in self.done:
            return
        self.done.add(name)
        doc = (node.get("description") or "").replace('"""', "'''").strip()
        if "properties" not in node:
            s, a = self.spec(node, name)
            self.out.append(f"{name} = {a}\n{name.upper()}_SPEC = {s}\n")
            return
        required = set(node.get("required", ()))
        lines, fields = [], []
        for prop, sub in node["properties"].items():
            sub = {} if isinstance(sub, bool) else sub
            s, a = self.spec(sub, name + pascal(prop))
            py = ident(prop)
            fields.append(f"        {py!r}: ({prop!r}, {s}),")
            if prop in required:
                if "const" in sub:
                    lines.append(f"    {py}: {a} = {sub['const']!r}")
                else:
                    lines.append(f"    {py}: {a}")
                continue
            default = sub.get("default")
            if default is None:
                a = a if a.startswith("Optional[") or a == "Any" else f"Optional[{a}]"
                lines.append(f"    {py}: {a} = None")
            elif isinstance(default, (list, dict)):
                lines.append(f"    {py}: {a} = field(default_factory=lambda: {default!r})")
            else:
                lines.append(f"    {py}: {a} = {default!r}")
        block = ["@dataclass(kw_only=True)", f"class {name}(Model):"]
        if doc:
            block.append(f'    """{doc}"""')
            block.append("")
        block += lines or ["    pass"]
        block.append("")
        block.append("    FIELDS: ClassVar[dict] = {")
        block += fields
        block.append("    }")
        if node.get("additionalProperties") is False:
            block.append("    CLOSED: ClassVar[bool] = True")
        self.out.append("\n".join(block) + "\n")


def render():
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    g = Gen(schema)
    g.run()
    head = [
        '"""The Hideout contract\'s wire types (schema/hideout-v1.json).',
        "",
        "GENERATED by tools/gen_hideout_models.py: do not edit; regenerate after the schema changes.",
        '"""',
        "",
        "from __future__ import annotations",
        "",
        "from dataclasses import dataclass, field",
        "from typing import Any, ClassVar, Literal, Optional, Union",
        "",
        "from ._model import Model",
        "",
        f"HUB = {schema['hub']!r}",
        f"SPEC = {schema['spec']!r}",
        "# The published icon names (unknown draws the generic 'module' icon) and tone words.",
        f"ICONS = {tuple(schema['icons'])!r}",
        f"TONES = {tuple(schema['tones'])!r}",
        f"TILE_LIMITS = {schema['tile_limits']!r}",
        "# Image scaling words (unknown draws 'smooth') and the panel stream rules (v1.5).",
        f"SCALINGS = {tuple(schema['scalings'])!r}",
        f"STREAMS = {schema['streams']!r}",
        "",
    ]
    types = ["", "# The schema's top-level types by their schema key.", "TYPES = {"]
    types += [f"    {key!r}: {cls}," for key, cls in sorted(g.top.items())]
    types.append("}")
    return "\n".join(head) + "\n\n".join(g.out) + "\n".join(types) + "\n"


def main():
    text = render()
    if "--check" in sys.argv:
        current = OUT.read_text(encoding="utf-8").replace("\r\n", "\n") if OUT.exists() else ""
        if current != text:
            print(f"{OUT.relative_to(ROOT)} is out of date: run python tools/gen_hideout_models.py")
            return 1
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
