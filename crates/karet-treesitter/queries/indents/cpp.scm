; karet indent query for C++: C's nodes plus templates.

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
  (template_parameter_list)
  (template_argument_list)
] @indent

["}" ")" "]" ">"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
