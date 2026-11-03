; Calls, bodies and comments for the seam call tier. C imports by textual inclusion,
; which binds no names, so there are no import captures here.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f()
(call_expression
  function: (identifier) @call.name) @call
; `ops->read(x)` calls through a function pointer: a call, but to nothing a name can
; find, so it is marked without one and counts as unresolved rather than external.
(call_expression
  function: (field_expression)) @call

; The standard library is always someone else's.
((call_expression
  function: (identifier) @call.name @call.external) @call
  (#any-of? @call.name
    "printf" "fprintf" "sprintf" "snprintf" "puts" "putchar" "fputs" "fgets" "scanf"
    "malloc" "calloc" "realloc" "free" "memcpy" "memmove" "memset" "memcmp" "strlen"
    "strcmp" "strncmp" "strcpy" "strncpy" "strcat" "strchr" "strrchr" "strstr" "strdup"
    "fopen" "fclose" "fread" "fwrite" "fflush" "exit" "abort" "assert" "atoi" "atol"
    "strtol" "strtoul" "strtod" "qsort" "bsearch" "abs" "getenv" "time" "sizeof"))

(function_definition
  body: (compound_statement) @body)

(comment) @comment
