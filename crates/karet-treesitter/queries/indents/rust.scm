; karet indent query for Rust. `@indent`: inner lines sit one level deeper than
; the line the node starts on. `@outdent`: a closing token.

[
  (block)
  (declaration_list)
  (field_declaration_list)
  (ordered_field_declaration_list)
  (enum_variant_list)
  (field_initializer_list)
  (match_block)
  (use_list)
  (arguments)
  (parameters)
  (closure_parameters)
  (type_arguments)
  (type_parameters)
  (array_expression)
  (tuple_expression)
  (tuple_type)
  (tuple_pattern)
  (slice_pattern)
  (token_tree)
] @indent

["}" ")" "]" ">"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
