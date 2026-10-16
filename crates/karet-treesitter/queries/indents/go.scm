; karet indent query for Go. A `case` clause has no closing token, so its body
; stays indented under it until the switch's `}`.

[
  (block)
  (literal_value)
  (argument_list)
  (parameter_list)
  (field_declaration_list)
  (interface_type)
  (expression_switch_statement)
  (type_switch_statement)
  (select_statement)
  (expression_case)
  (default_case)
  (type_case)
  (communication_case)
  (import_spec_list)
  (var_spec_list)
] @indent

; Only the parenthesised group form of `const (…)`.
(const_declaration "(") @indent

["}" ")" "]"] @outdent

; Unbalanced code being typed parses as an error, not a block.
(ERROR ["{" "(" "["] @open)
