; karet fold query for Go. `@fold`: a construct that folds from the line it
; starts on through the line it ends on. `@fold.comment` / `@fold.imports`: one
; comment / import declaration; a run of them on adjacent lines folds as one.

[
  (function_declaration)
  (method_declaration)
  (func_literal)
  (block)
  (type_declaration)
  (field_declaration_list)
  (interface_type)
  (composite_literal)
  (literal_value)
  (argument_list)
  (parameter_list)
  (expression_switch_statement)
  (type_switch_statement)
  (select_statement)
  (expression_case)
  (type_case)
  (communication_case)
  (default_case)
  (const_declaration)
  (var_declaration)
  (raw_string_literal)
] @fold

(comment) @fold.comment

(import_declaration) @fold.imports
