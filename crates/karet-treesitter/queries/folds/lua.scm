; karet fold query for Lua. `@fold`: a construct that folds from the line it
; starts on through the line it ends on. `@fold.comment`: one comment; a run of
; them on adjacent lines folds as one.

[
  (function_declaration)
  (function_definition)
  (if_statement)
  (elseif_statement)
  (else_statement)
  (do_statement)
  (while_statement)
  (repeat_statement)
  (for_statement)
  (table_constructor)
  (arguments)
  (parameters)
] @fold

(comment) @fold.comment
