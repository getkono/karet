; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), ns::f(), a::b::f(), f<T>(), x.f(), p->f(), this->f(), new T()
(call_expression
  function: (identifier) @call.name) @call
(call_expression
  function: (qualified_identifier
    scope: (_) @call.qualifier
    name: (identifier) @call.name)) @call
; For `a::b::f` the scope nearest the name is the one that narrows.
(call_expression
  function: (qualified_identifier
    name: (qualified_identifier
      scope: (_) @call.qualifier
      name: (identifier) @call.name))) @call
(call_expression
  function: (template_function
    name: (identifier) @call.name)) @call
(call_expression
  function: (qualified_identifier
    scope: (_) @call.qualifier
    name: (template_function
      name: (identifier) @call.name))) @call
(call_expression
  function: (field_expression
    argument: (this) @call.self
    field: (field_identifier) @call.name)) @call
(call_expression
  function: (field_expression
    argument: (_) @call.receiver
    field: (field_identifier) @call.name)) @call
(new_expression
  type: (type_identifier) @call.name @call.new) @call

; The C library is always someone else's.
((call_expression
  function: (identifier) @call.name @call.external) @call
  (#any-of? @call.name
    "printf" "fprintf" "snprintf" "puts" "malloc" "calloc" "realloc" "free" "memcpy"
    "memmove" "memset" "memcmp" "strlen" "strcmp" "strncmp" "exit" "abort" "assert"
    "move" "forward" "swap" "make_unique" "make_shared" "static_cast" "dynamic_cast"))

(function_definition
  body: (compound_statement) @body)

(comment) @comment

(using_declaration
  (qualified_identifier) @import.path)
