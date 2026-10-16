; karet indent query for Python. A compound statement's body has no closing
; token, so the statement itself is the `@indent` node: a caret at the end of its
; last line is still inside it.

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
  (argument_list)
  (parameters)
  (list)
  (dictionary)
  (set)
  (tuple)
  (parenthesized_expression)
  (generator_expression)
  (list_comprehension)
  (dictionary_comprehension)
  (set_comprehension)
] @indent

["}" ")" "]"] @outdent

; Unbalanced code being typed (`def f():` with no body yet) parses as an error.
(ERROR [":" "(" "[" "{"] @open)
