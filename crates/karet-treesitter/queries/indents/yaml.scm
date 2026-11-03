; karet indent query for YAML. A pair indents only when its value is a nested
; block, or when it has no value yet (`key:` and Enter); `key: scalar` does not,
; so the next line is its sibling.
;
; A sequence is not an indent node, nor is a pair whose value is one: YAML lets
; a sequence sit at its key's own column (`steps:` then `- run: x`), and an
; item's continuation aligns after its `- ` rather than one level deeper, so
; Enter there keeps the current line's column.

(block_mapping_pair value: (block_node (block_mapping))) @indent
(block_mapping_pair !value) @indent

[(flow_mapping) (flow_sequence)] @indent

["}" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "["] @open)
