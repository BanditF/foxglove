// Checks Foxglove.js's E1.31 packets against the receiver's parser rules (crates/plugin-openrgb-e131
// e131::parse), and the plugin's send-on-change behaviour, with SignalRGB's globals stubbed.
// Run: node integrations/signalrgb/test.mjs
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "Foxglove.js"), "utf8").replace(
	'import { udp } from "@SignalRGB/udp";',
	"const udp = globalThis.__udp;",
);
const sent = [];
globalThis.__udp = { createSocket: () => ({ write: (data, host, port) => sent.push({ data, host, port }), close: () => {} }) };
let colour = [255, 0, 0];
globalThis.device = { setName() {}, setSize() {}, setControllableLeds() {}, color: () => colour };
globalThis.controller = { name: "Foxglove keyboard" };
const file = join(mkdtempSync(join(tmpdir(), "foxglove-srgb-")), "plugin.mjs");
writeFileSync(file, source);
const p = await import(pathToFileURL(file));

// The receiver's checks.
function parse(b) {
	const u16 = (i) => (b[i] << 8) | b[i + 1];
	const u32 = (i) => ((b[i] << 24) >>> 0) + (b[i + 1] << 16) + (b[i + 2] << 8) + b[i + 3];
	assert.ok(b.length >= 127);
	assert.equal(u16(0), 0x0010);
	assert.equal(u16(2), 0);
	assert.equal(String.fromCharCode(...b.slice(4, 16)), "ASC-E1.17\0\0\0");
	assert.equal(u32(18), 4);
	assert.equal(u32(40), 2);
	assert.equal(b[117], 0x02);
	assert.equal(b[118], 0xa1);
	assert.equal(b[126], 0, "start code 0");
	const count = u16(123);
	assert.equal(126 + count, b.length);
	// The PDU lengths match the packet.
	assert.equal(u16(16) & 0x0fff, b.length - 16);
	assert.equal(u16(38) & 0x0fff, b.length - 38);
	assert.equal(u16(115) & 0x0fff, b.length - 115);
	return { universe: u16(113), sequence: b[111], terminated: (b[112] & 0x40) !== 0, channels: b.slice(127) };
}

const pkt = parse(p.e131Packet(7, [10, 20, 30], false));
assert.deepEqual(pkt, { universe: 1, sequence: 7, terminated: false, channels: [10, 20, 30] });
assert.equal(parse(p.e131Packet(300, [0, 0, 0], true)).sequence, 300 & 0xff);
assert.ok(parse(p.e131Packet(1, [0, 0, 0], true)).terminated);

// The plugin: a frame on change only, to 127.0.0.1:5568; the stop on Shutdown.
p.Initialize();
p.Render();
p.Render();
assert.equal(sent.length, 1, "the same colour again sends nothing");
colour = [0, 255, 0];
p.Render();
assert.equal(sent.length, 2);
assert.deepEqual(parse(sent[1].data).channels, [0, 255, 0]);
assert.deepEqual([sent[1].host, sent[1].port], ["127.0.0.1", 5568]);
p.Shutdown();
assert.ok(parse(sent[2].data).terminated, "Shutdown ends the stream");
console.log("ok");
