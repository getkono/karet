; karet indent query for YAML. A pair indents only when its value is a nested
; block, or when it has no value yet (`key:` and Enter); `key: scalar` does not,
; so the next line is its sibling.

(block_mapping_pair value: (block_node)) @indent
(block_mapping_pair !value) @indent
(block_sequence_item (block_node)) @indent

[(flow_mapping) (flow_sequence)] @indent

["}" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "["] @open)
