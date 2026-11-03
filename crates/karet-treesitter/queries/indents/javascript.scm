; karet indent query for JavaScript (and JSX).

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
] @indent

["}" ")" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
