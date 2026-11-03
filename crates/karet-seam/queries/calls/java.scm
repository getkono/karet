; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), x.f(), this.f(), super.f(), new T()
(method_invocation
  name: (identifier) @call.name) @call
(method_invocation
  object: (this) @call.self
  name: (identifier) @call.name) @call
(method_invocation
  object: (super) @call.super
  name: (identifier) @call.name) @call
(method_invocation
  object: (_) @call.receiver
  name: (identifier) @call.name) @call
(object_creation_expression
  type: (type_identifier) @call.name @call.new) @call

(method_declaration
  body: (block) @body)
(constructor_declaration
  body: (constructor_body) @body)

(line_comment) @comment
(block_comment) @comment

(import_declaration
  (scoped_identifier) @import.path
  .)
(import_declaration
  (scoped_identifier) @import.glob
  (asterisk))
