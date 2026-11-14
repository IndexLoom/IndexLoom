/** Incremental SSE parser: handles split UTF-8 chunks, CRLF, comments and multiline data. */
export class SseParser {
  private buffer = "";
  private readonly receive: (event: string, id: string, data: string) => void;
  constructor(receive: (event: string, id: string, data: string) => void) { this.receive = receive; }
  push(text: string) {
    this.buffer += text;
    if (this.buffer.length > 65536) throw new Error("Event frame exceeds limit");
    for (;;) {
      const boundary = /\r?\n\r?\n/.exec(this.buffer);
      if (!boundary) break;
      const frame = this.buffer.slice(0, boundary.index);
      this.buffer = this.buffer.slice(boundary.index + boundary[0].length);
      let type = "message", id = ""; const data: string[] = [];
      for (const line of frame.split(/\r?\n/)) {
        if (line.startsWith(":")) continue;
        const colon = line.indexOf(":");
        const key = colon < 0 ? line : line.slice(0, colon);
        const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (key === "event") type = value;
        if (key === "id" && !value.includes("\0")) id = value;
        if (key === "data") data.push(value);
      }
      if (data.length) this.receive(type, id, data.join("\n"));
    }
  }
}
