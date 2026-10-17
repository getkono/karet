; karet fold query for YAML. `@fold`: a key, sequence item or flow collection
; folds from the line it starts on through the line it ends on.
; `@fold.comment`: one comment; a run of them on adjacent lines folds as one.

[
  (block_mapping_pair)
  (block_sequence_item)
  (flow_mapping)
  (flow_sequence)
] @fold

(comment) @fold.comment
