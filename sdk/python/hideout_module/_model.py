"""The runtime behind the generated models: decode and encode by a small type spec.

A spec is 'str', 'int', 'num', 'bool', 'any', ('list', s), ('map', s), ('opt', s),
('enum', values), ('const', value), ('ref', name) or ('union', tag, {value: class name}).
"""

import sys
from dataclasses import fields as dc_fields


class ModelError(ValueError):
    """A value that doesn't fit the contract's type."""


def _resolve(name):
    module = sys.modules[__package__ + ".models"]
    target = getattr(module, name, None)
    if isinstance(target, type) and issubclass(target, Model):
        return target
    return getattr(module, name.upper() + "_SPEC")


def decode(spec, value, where="value"):
    if spec == "any":
        return value
    if spec == "str":
        if not isinstance(value, str):
            raise ModelError(f"{where}: a string")
        return value
    if spec == "int":
        if not isinstance(value, int) or isinstance(value, bool):
            raise ModelError(f"{where}: an integer")
        return value
    if spec == "num":
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise ModelError(f"{where}: a number")
        return value
    if spec == "bool":
        if not isinstance(value, bool):
            raise ModelError(f"{where}: true or false")
        return value
    kind = spec[0]
    if kind == "opt":
        return None if value is None else decode(spec[1], value, where)
    if kind == "list":
        if not isinstance(value, list):
            raise ModelError(f"{where}: a list")
        return [decode(spec[1], v, f"{where}[{i}]") for i, v in enumerate(value)]
    if kind == "map":
        if not isinstance(value, dict):
            raise ModelError(f"{where}: an object")
        return {k: decode(spec[1], v, f"{where}.{k}") for k, v in value.items()}
    if kind in ("enum", "const"):
        allowed = spec[1] if kind == "enum" else (spec[1],)
        if value not in allowed:
            raise ModelError(f"{where}: one of {', '.join(map(str, allowed))}")
        return value
    if kind == "ref":
        target = _resolve(spec[1])
        if isinstance(target, type):
            return target.from_json(value, where)
        return decode(target, value, where)
    if kind == "union":
        if not isinstance(value, dict) or value.get(spec[1]) not in spec[2]:
            raise ModelError(f"{where}: {spec[1]} is one of {', '.join(spec[2])}")
        return _resolve(spec[2][value[spec[1]]]).from_json(value, where)
    raise ModelError(f"unknown spec {spec!r}")


def encode(value):
    if isinstance(value, Model):
        return value.to_json()
    if isinstance(value, list):
        return [encode(v) for v in value]
    if isinstance(value, dict):
        return {k: encode(v) for k, v in value.items()}
    return value


class Model:
    """A generated wire type. `FIELDS` maps a Python field to (JSON key, spec)."""

    FIELDS = {}
    CLOSED = False

    @classmethod
    def from_json(cls, data, where=None):
        where = where or cls.__name__
        if not isinstance(data, dict):
            raise ModelError(f"{where}: an object")
        known = {key for key, _ in cls.FIELDS.values()}
        if cls.CLOSED and set(data) - known:
            raise ModelError(f"{where}: unknown keys {sorted(set(data) - known)}")
        args = {}
        for py, (key, spec) in cls.FIELDS.items():
            if key in data:
                args[py] = decode(spec, data[key], f"{where}.{key}")
            elif not _has_default(cls, py):
                raise ModelError(f"{where}: {key} is missing")
        return cls(**args)

    def to_json(self):
        """The wire form; a field left at None is left off (it reads back as its default)."""
        out = {}
        for py, (key, _) in self.FIELDS.items():
            value = getattr(self, py)
            if value is not None:
                out[key] = encode(value)
        return out


def _has_default(cls, name):
    from dataclasses import MISSING

    for f in dc_fields(cls):
        if f.name == name:
            return f.default is not MISSING or f.default_factory is not MISSING
    return False
