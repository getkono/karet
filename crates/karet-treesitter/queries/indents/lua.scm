; karet indent query for Lua. Block statements close with `end` (or `until`),
; which lands back at the statement's own level.

[
  (function_declaration)
  (function_definition)
  (if_statement)
  (elseif_statement)
  (else_statement)
  (for_statement)
  (while_statement)
  (repeat_statement)
  (do_statement)
  (table_constructor)
  (arguments)
  (parameters)
] @indent

["end" "until" "}" ")"] @outdent

; Unbalanced code being typed (`function f()` with no `end` yet) parses as an error.
(ERROR ["function" "then" "do" "repeat" "else" "{" "("] @open)
