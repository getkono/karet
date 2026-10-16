; karet indent query for C. A `case` label's statements have no closing token,
; so they stay indented under it until the switch body's `}`.

[
  (compound_statement)
  (field_declaration_list)
  (enumerator_list)
  (initializer_list)
  (declaration_list)
  (argument_list)
  (parameter_list)
  (parenthesized_expression)
  (case_statement)
] @indent

["}" ")" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
