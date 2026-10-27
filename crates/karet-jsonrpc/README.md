# karet-jsonrpc

> Protocol-agnostic async JSON-RPC 2.0 client core: request/response correlation
> over pluggable framing.

The transport half of a JSON-RPC 2.0 client, with **zero karet dependencies**: outgoing
request/notification/response envelopes, shape-based classification of inbound messages,
and a connection actor that owns a reader task and a writer task — allocating request ids,
correlating responses through a pending map, bounding every request with a timeout, draining
a bounded outbound queue, failing all in-flight requests on EOF, and closing politely.

Everything protocol-specific lives behind three traits: `Framing` (the wire framing —
`ContentLength` for the LSP base protocol, `LineDelimited` for newline-delimited JSON),
`Envelope` (the request id type, request encoding, and how an inbound message classifies),
and `Handler` (for JSON-RPC: the broadcast payload, peer-request answers, and notification
side effects). The correlation `Actor` is generic over `Envelope`, so a protocol that is not
JSON-RPC — DAP, whose responses carry a `request_seq` — reuses the same pending map,
timeouts, bounded queue and fail-all-on-EOF; `Connection` is that actor speaking JSON-RPC.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
