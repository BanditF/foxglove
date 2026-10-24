// Foxglove for SignalRGB (#73): a SignalRGB network plugin that sends the canvas colour to
// Foxglove's E1.31 receiver on this PC (127.0.0.1:5568, universe 1). Foxglove holds the
// keyboard's underglow with a low-priority lease while frames come, so its own notifications
// still show, and gives the lights back when they stop.
//
// Whole-board colour only: one sample of the canvas, sent when it changes (and once a second,
// so Foxglove knows the stream is still there). Nothing leaves this PC.
//
// Install: copy this file to Documents\WhirlwindFX\Plugins, restart SignalRGB, and turn on
// Foxglove's "OpenRGB (E1.31)" plugin (the receiver). Not made or endorsed by SignalRGB.
//
// Copyright (c) 2026 BanditF. MIT licence (see the Foxglove repository's LICENSE).

import { udp } from "@SignalRGB/udp";

const HOST = "127.0.0.1";
const PORT = 5568;
const UNIVERSE = 1;
/// A frame is sent at least this often (ms) while SignalRGB runs, inside E1.31's 2.5 s loss
/// timeout; otherwise only when the colour changes.
const KEEP_ALIVE_MS = 1000;

export function Name() { return "Foxglove"; }
export function Version() { return "1.0.0"; }
export function Type() { return "network"; }
export function Publisher() { return "Foxglove"; }
export function Size() { return [1, 1]; }
export function DefaultPosition() { return [0, 0]; }
export function DefaultScale() { return 8.0; }
export function ControllableParameters() { return []; }

// One device, always on this PC: announced once, nothing to discover or poll.
export function DiscoveryService() {
	this.IconUrl = "";
	this.Initialize = function () {
		if (!service.hasController("foxglove")) {
			const c = new FoxgloveController();
			service.addController(c);
			service.announceController(c);
		}
	};
	this.Update = function () {};
}

class FoxgloveController {
	constructor() {
		this.id = "foxglove";
		this.name = "Foxglove keyboard";
	}
}

// The sender's E1.31 component id: fixed, so Foxglove counts sequence numbers per sender.
const CID = [0x66, 0x6f, 0x78, 0x67, 0x6c, 0x6f, 0x76, 0x65, 0x2d, 0x73, 0x72, 0x67, 0x62, 0x00, 0x00, 0x01];

/// An E1.31 (sACN) data packet: DMX channels `channels` on `universe`. `stop` sets the
/// stream-terminated bit.
export function e131Packet(sequence, channels, stop) {
	const DATA = 126;
	const count = channels.length + 1;
	const total = DATA + count;
	const b = new Array(total).fill(0);
	const u16 = (at, v) => { b[at] = (v >> 8) & 0xff; b[at + 1] = v & 0xff; };
	const u32 = (at, v) => { u16(at, (v >>> 16) & 0xffff); u16(at + 2, v & 0xffff); };
	const flagsLen = (at, len) => u16(at, 0x7000 | len);
	u16(0, 0x0010);
	"ASC-E1.17".split("").forEach((c, i) => { b[4 + i] = c.charCodeAt(0); });
	flagsLen(16, total - 16);
	u32(18, 0x00000004);
	CID.forEach((v, i) => { b[22 + i] = v; });
	flagsLen(38, total - 38);
	u32(40, 0x00000002);
	"Foxglove for SignalRGB".split("").forEach((c, i) => { b[44 + i] = c.charCodeAt(0); });
	b[108] = 100;
	b[111] = sequence & 0xff;
	b[112] = stop ? 0x40 : 0;
	u16(113, UNIVERSE);
	flagsLen(115, total - 115);
	b[117] = 0x02;
	b[118] = 0xa1;
	u16(121, 1);
	u16(123, count);
	channels.forEach((v, i) => { b[DATA + 1 + i] = v & 0xff; });
	return b;
}

let socket = null;
let sequence = 0;
let last = null;
let lastSent = 0;

function send(rgb, stop) {
	sequence = (sequence + 1) & 0xff;
	socket.write(e131Packet(sequence, rgb, stop), HOST, PORT);
	lastSent = Date.now();
}

export function Initialize() {
	device.setName(controller.name);
	device.setSize([1, 1]);
	device.setControllableLeds(["Underglow"], [[0, 0]]);
	socket = udp.createSocket();
}

// SignalRGB calls this each frame; a frame goes out only on a change or the keep-alive.
export function Render() {
	const c = device.color(0, 0);
	const rgb = [c[0], c[1], c[2]];
	const changed = last === null || rgb.some((v, i) => v !== last[i]);
	if (changed || Date.now() - lastSent >= KEEP_ALIVE_MS) {
		send(rgb, false);
		last = rgb;
	}
}

// SignalRGB closing or the plugin unloading: end the stream, so Foxglove gives the lights back
// at once instead of after its loss timeout.
export function Shutdown() {
	if (socket !== null) {
		send([0, 0, 0], true);
		socket.close();
		socket = null;
	}
}
