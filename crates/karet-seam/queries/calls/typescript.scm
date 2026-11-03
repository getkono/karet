; Calls, bodies, comments and imports for the seam call tier.
; Compiled against the JavaScript, TypeScript and TSX grammars alike, so it names only
; nodes all three share. Capture names: crates/karet-seam/src/calls/site.rs.

; f(), x.f(), this.f(), super.f(), x.#f(), new X()
(call_expression
  function: (identifier) @call.name) @call
(call_expression
  function: (member_expression
    object: (this) @call.self
    property: [(property_identifier) (private_property_identifier)] @call.name)) @call
(call_expression
  function: (member_expression
    object: (super) @call.super
    property: (property_identifier) @call.name)) @call
(call_expression
  function: (member_expression
    object: (_) @call.receiver
    property: [(property_identifier) (private_property_identifier)] @call.name)) @call
(new_expression
  constructor: (identifier) @call.name @call.new) @call

; The platform's globals are always someone else's.
((call_expression
  function: (identifier) @call.name @call.external) @call
  (#any-of? @call.name
    "require" "parseInt" "parseFloat" "isNaN" "isFinite" "setTimeout" "setInterval"
    "clearTimeout" "clearInterval" "queueMicrotask" "structuredClone" "fetch" "String"
    "Number" "Boolean" "Symbol" "BigInt" "Array" "Object" "Promise" "Error"))

(function_declaration
  body: (statement_block) @body)
(generator_function_declaration
  body: (statement_block) @body)
(method_definition
  body: (statement_block) @body)
(function_expression
  body: (statement_block) @body)
(arrow_function
  body: (_) @body)

(comment) @comment

(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @import.path
        alias: (identifier)? @import.alias)))
  source: (string) @import.prefix)
(import_statement
  (import_clause
    (namespace_import
      (identifier) @import.alias))
  source: (string) @import.path)
