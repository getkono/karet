; karet fold query for TypeScript and TSX (shared, so it names no JSX node).
; `@fold`: a construct that folds from the line it starts on through the line
; it ends on. `@fold.comment` / `@fold.imports`: one comment / import
; statement; a run of them on adjacent lines folds as one.

[
  (function_declaration)
  (function_expression)
  (generator_function_declaration)
  (generator_function)
  (arrow_function)
  (method_definition)
  (class_declaration)
  (abstract_class_declaration)
  (class_body)
  (class_static_block)
  (interface_declaration)
  (interface_body)
  (enum_declaration)
  (enum_body)
  (type_alias_declaration)
  (object_type)
  (internal_module)
  (module)
  (statement_block)
  (switch_body)
  (switch_case)
  (switch_default)
  (object)
  (object_pattern)
  (array)
  (array_pattern)
  (arguments)
  (formal_parameters)
  (type_parameters)
  (type_arguments)
  (template_string)
  (named_imports)
  (export_clause)
] @fold

(comment) @fold.comment

(import_statement) @fold.imports
