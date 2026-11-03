; karet fold query for JavaScript (JSX included). `@fold`: a construct that
; folds from the line it starts on through the line it ends on.
; `@fold.comment` / `@fold.imports`: one comment / import statement; a run of
; them on adjacent lines folds as one.

[
  (function_declaration)
  (function_expression)
  (generator_function_declaration)
  (generator_function)
  (arrow_function)
  (method_definition)
  (class_declaration)
  (class_body)
  (class_static_block)
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
  (template_string)
  (named_imports)
  (export_clause)
  (jsx_element)
  (jsx_self_closing_element)
] @fold

(comment) @fold.comment

(import_statement) @fold.imports
