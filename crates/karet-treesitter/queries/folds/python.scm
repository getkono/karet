; karet fold query for Python. `@fold`: a construct that folds from the line it
; starts on through the line it ends on. `@fold.comment` / `@fold.imports`: one
; comment / import statement; a run of them on adjacent lines folds as one.

[
  (function_definition)
  (class_definition)
  (if_statement)
  (elif_clause)
  (else_clause)
  (for_statement)
  (while_statement)
  (try_statement)
  (except_clause)
  (finally_clause)
  (with_statement)
  (match_statement)
  (case_clause)
  (dictionary)
  (list)
  (tuple)
  (set)
  (list_comprehension)
  (dictionary_comprehension)
  (set_comprehension)
  (generator_expression)
  (argument_list)
  (parameters)
  (parenthesized_expression)
  (string)
] @fold

(comment) @fold.comment

[
  (import_statement)
  (import_from_statement)
  (future_import_statement)
] @fold.imports
