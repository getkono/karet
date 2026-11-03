; karet indent query for JSON.

[(object) (array)] @indent

["}" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "["] @open)
