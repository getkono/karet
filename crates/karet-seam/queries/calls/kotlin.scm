; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), x.f(), this.f(), super.f(). A call to a class's name is its constructor, which
; resolution finds by name, so `Widget()` needs no pattern of its own.
(call_expression
  .
  (identifier) @call.name) @call
(call_expression
  .
  (navigation_expression
    (this_expression) @call.self
    .
    (identifier) @call.name .)) @call
(call_expression
  .
  (navigation_expression
    (super_expression) @call.super
    .
    (identifier) @call.name .)) @call
(call_expression
  .
  (navigation_expression
    (_) @call.receiver
    .
    (identifier) @call.name .)) @call

((call_expression
  .
  (identifier) @call.name @call.external) @call
  (#any-of? @call.name
    "println" "print" "require" "requireNotNull" "check" "checkNotNull" "error" "TODO"
    "listOf" "mutableListOf" "setOf" "mutableSetOf" "mapOf" "mutableMapOf" "arrayOf"
    "emptyList" "emptyMap" "emptySet" "lazy" "run" "let" "also" "apply" "with"
    "repeat" "maxOf" "minOf" "buildList" "buildMap" "buildString" "Pair" "Triple"))

(function_declaration
  (function_body) @body)
(secondary_constructor
  (block) @body)

(line_comment) @comment
(block_comment) @comment

(import
  (qualified_identifier) @import.path
  .
  (identifier)? @import.alias)
