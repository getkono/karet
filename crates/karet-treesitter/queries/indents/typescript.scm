; karet indent query for TypeScript and TSX: JavaScript's nodes plus the
; type-level ones TypeScript adds.

[
  (statement_block)
  (class_body)
  (switch_body)
  (switch_case)
  (switch_default)
  (object)
  (object_pattern)
  (array)
  (array_pattern)
  (arguments)
  (formal_parameters)
  (named_imports)
  (export_clause)
  (parenthesized_expression)
  (template_substitution)
  (interface_body)
  (object_type)
  (enum_body)
  (type_parameters)
  (type_arguments)
  (tuple_type)
] @indent

["}" ")" "]" ">"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
