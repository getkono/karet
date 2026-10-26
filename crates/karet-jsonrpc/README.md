# karet-jsonrpc

> Protocol-agnostic async JSON-RPC 2.0 client core: request/response correlation
> over pluggable framing.

The transport half of a JSON-RPC 2.0 client, with **zero karet dependencies**: outgoing
request/notification/response envelopes, shape-based classification of inbound messages,
and a connection actor that owns a reader task and a writer task — allocating request ids,
correlating responses through a pending map, bounding every request with a timeout, draining
a bounded outbound queue, failing all in-flight requests on EOF, and closing politely.

Everything protocol-specific lives behind two traits: `Framing` (the wire framing —
`ContentLength` for the LSP base protocol, `LineDelimited` for newline-delimited JSON)
and `Handler` (the broadcast payload, peer-request answers, and notification side effects).
DAP is not JSON-RPC, but its framing is the same `Content-Length` header, so a DAP client
can reuse `ContentLength` on its own.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
