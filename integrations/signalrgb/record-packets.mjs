// Records the E1.31 packets Foxglove.js sends in a short SignalRGB session (red, red again, green,
// SignalRGB closing), with SignalRGB's globals stubbed as in test.mjs, so the receiver's end-to-end
// test (crates/plugin-openrgb-e131/tests/signalrgb.rs) replays exactly what the plugin sends.
// The file is each packet as a big-endian u16 length and its bytes.
//   node integrations/signalrgb/record-packets.mjs           writes the fixture
//   node integrations/signalrgb/record-packets.mjs --check   fails if Foxglove.js no longer matches it
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = join(here, "../../crates/plugin-openrgb-e131/tests/fixtures/signalrgb-session.bin");
const source = readFileSync(join(here, "Foxglove.js"), "utf8").replace(
	'import { udp } from "@SignalRGB/udp";',
	"const udp = globalThis.__udp;",
);
const sent = [];
globalThis.__udp = { createSocket: () => ({ write: (data) => sent.push(data), close: () => {} }) };
let colour = [255, 0, 0];
globalThis.device = { setName() {}, setSize() {}, setControllableLeds() {}, color: () => colour };
globalThis.controller = { name: "Foxglove keyboard" };
const file = join(mkdtempSync(join(tmpdir(), "foxglove-srgb-")), "plugin.mjs");
writeFileSync(file, source);
const p = await import(pathToFileURL(file));

p.Initialize();
p.Render(); // red
p.Render(); // the same colour: nothing
colour = [0, 255, 0];
p.Render(); // green
p.Shutdown(); // the stream ends

const parts = sent.map((d) => Buffer.concat([Buffer.from([(d.length >> 8) & 0xff, d.length & 0xff]), Buffer.from(d)]));
const bytes = Buffer.concat(parts);
if (process.argv.includes("--check")) {
	if (!readFileSync(fixture).equals(bytes)) {
		console.error("signalrgb-session.bin is out of date: run node integrations/signalrgb/record-packets.mjs");
		process.exit(1);
	}
	console.log(`ok: ${sent.length} packets match the fixture`);
} else {
	writeFileSync(fixture, bytes);
	console.log(`wrote ${sent.length} packets to ${fixture}`);
}
