// Bundled ahead of the wasm-bindgen glue, and only ever loaded inside a worklet.
//
// `AudioWorkletGlobalScope` is a deliberately tiny realm: no `document`, no `fetch`, no
// `location` — and no `TextDecoder`. wasm-bindgen's glue constructs one at the top of its
// IIFE, unconditionally, the moment the bundle is evaluated, so without this the whole
// bundle dies with `ReferenceError: TextDecoder is not defined` before `registerProcessor`
// is ever reached. The failure is invisible from the page: `addModule()` resolves, and
// only the later `new AudioWorkletNode(...)` complains that the processor name is not
// defined.
//
// The glue needs exactly one thing from it — decode a UTF-8 byte range into a string —
// and in this host the only strings that cross out of wasm are load-error messages. So
// this is a decoder, not a `TextDecoder`: same constructor shape and same `decode`
// contract, deliberately no `encode` side. `cargo xtask wasm` fails the build if the glue
// ever starts wanting `TextEncoder`, rather than letting that fail in a browser.

'use strict';

if (typeof TextDecoder === 'undefined') {
    globalThis.TextDecoder = class TextDecoder {
        constructor(label = 'utf-8', options = {}) {
            const encoding = String(label).toLowerCase();
            if (encoding !== 'utf-8' && encoding !== 'utf8') {
                throw new RangeError(`the worklet decoder only implements utf-8, not ${label}`);
            }
            this.encoding = 'utf-8';
            this.fatal = options.fatal === true;
            this.ignoreBOM = options.ignoreBOM === true;
        }

        decode(input) {
            if (input === undefined) return '';
            const bytes = input instanceof Uint8Array ? input : new Uint8Array(input.buffer ?? input, input.byteOffset ?? 0, input.byteLength ?? input.length);
            // Assembled in chunks: `String.fromCharCode.apply` on a whole module title is
            // fine, but on a multi-kilobyte string it risks the argument limit.
            const units = [];
            let text = '';
            let index = 0;
            while (index < bytes.length) {
                const first = bytes[index];
                let codePoint;
                let continuationCount;
                if (first < 0x80) {
                    codePoint = first;
                    continuationCount = 0;
                } else if ((first & 0xe0) === 0xc0) {
                    codePoint = first & 0x1f;
                    continuationCount = 1;
                } else if ((first & 0xf0) === 0xe0) {
                    codePoint = first & 0x0f;
                    continuationCount = 2;
                } else if ((first & 0xf8) === 0xf0) {
                    codePoint = first & 0x07;
                    continuationCount = 3;
                } else {
                    codePoint = -1;
                    continuationCount = 0;
                }
                index += 1;
                for (let step = 0; step < continuationCount; step += 1) {
                    const continuation = bytes[index];
                    if (continuation === undefined || (continuation & 0xc0) !== 0x80) {
                        codePoint = -1;
                        break;
                    }
                    codePoint = (codePoint << 6) | (continuation & 0x3f);
                    index += 1;
                }
                if (codePoint < 0 || codePoint > 0x10ffff || (codePoint >= 0xd800 && codePoint <= 0xdfff)) {
                    if (this.fatal) throw new TypeError('the worklet decoder found malformed UTF-8');
                    codePoint = 0xfffd;
                }
                if (codePoint > 0xffff) {
                    const offset = codePoint - 0x10000;
                    units.push(0xd800 + (offset >> 10), 0xdc00 + (offset & 0x3ff));
                } else {
                    units.push(codePoint);
                }
                if (units.length >= 1024) {
                    text += String.fromCharCode(...units);
                    units.length = 0;
                }
            }
            if (units.length !== 0) text += String.fromCharCode(...units);
            return !this.ignoreBOM && text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
        }
    };
}
