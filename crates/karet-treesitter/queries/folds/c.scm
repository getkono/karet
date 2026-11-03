; karet fold query for C. `@fold`: a construct that folds from the line it
; starts on through the line it ends on. `@fold.comment` / `@fold.imports`: one
; comment / `#include`; a run of them on adjacent lines folds as one.

[
  (function_definition)
  (compound_statement)
  (struct_specifier)
  (union_specifier)
  (enum_specifier)
  (field_declaration_list)
  (enumerator_list)
  (initializer_list)
  (argument_list)
  (parameter_list)
  (case_statement)
  (preproc_if)
  (preproc_ifdef)
  (preproc_elif)
  (preproc_else)
  (preproc_function_def)
] @fold

(comment) @fold.comment

(preproc_include) @fold.imports
