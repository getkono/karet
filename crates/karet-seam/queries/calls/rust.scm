; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), a::b::f(), x.f(), self.f(), and the turbofish forms of each.
(call_expression
  function: (identifier) @call.name) @call
(call_expression
  function: (scoped_identifier
    path: (_) @call.qualifier
    name: (identifier) @call.name)) @call
(call_expression
  function: (field_expression
    value: (self) @call.self
    field: (field_identifier) @call.name)) @call
(call_expression
  function: (field_expression
    value: (_) @call.receiver
    field: (field_identifier) @call.name)) @call
(call_expression
  function: (generic_function
    function: (identifier) @call.name)) @call
(call_expression
  function: (generic_function
    function: (scoped_identifier
      path: (_) @call.qualifier
      name: (identifier) @call.name))) @call
(call_expression
  function: (generic_function
    function: (field_expression
      value: (_) @call.receiver
      field: (field_identifier) @call.name))) @call

; The prelude's constructors and functions are always someone else's.
((call_expression
  function: (identifier) @call.name @call.external) @call
  (#any-of? @call.name "Some" "Ok" "Err" "Box" "drop" "panic" "Vec" "String"))

; A macro's arguments are tokens, not syntax: `name (` inside one is probably a call.
(token_tree
  (identifier) @call.name @call
  .
  (token_tree) @call.weak)

(function_item
  body: (block) @body)

(line_comment) @comment
(block_comment) @comment

(use_declaration
  argument: [(identifier) (scoped_identifier)] @import.path)
(use_declaration
  argument: (use_as_clause
    path: (_) @import.path
    alias: (identifier) @import.alias))
(scoped_use_list
  path: (_) @import.prefix
  list: (use_list
    [(identifier) (scoped_identifier)] @import.path))
(scoped_use_list
  path: (_) @import.prefix
  list: (use_list
    (use_as_clause
      path: (_) @import.path
      alias: (identifier) @import.alias)))
(use_wildcard
  (_) @import.glob)
