; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), x.f(), self.f(), super.f(). A call to a type's name is its initializer, which
; resolution finds by name, so `Widget()` needs no pattern of its own.
(call_expression
  (simple_identifier) @call.name
  (call_suffix)) @call
(call_expression
  (navigation_expression
    target: (self_expression) @call.self
    suffix: (navigation_suffix
      suffix: (simple_identifier) @call.name))
  (call_suffix)) @call
(call_expression
  (navigation_expression
    target: (super_expression) @call.super
    suffix: (navigation_suffix
      suffix: (simple_identifier) @call.name))
  (call_suffix)) @call
(call_expression
  (navigation_expression
    target: (_) @call.receiver
    suffix: (navigation_suffix
      suffix: (simple_identifier) @call.name))
  (call_suffix)) @call

; The grammar reads `a + f()` as `(a + f)()`: the callee is the right operand.
(call_expression
  (_
    rhs: (simple_identifier) @call.name)
  (call_suffix)) @call
(call_expression
  (_
    rhs: (navigation_expression
      target: (_) @call.receiver
      suffix: (navigation_suffix
        suffix: (simple_identifier) @call.name)))
  (call_suffix)) @call

((call_expression
  (simple_identifier) @call.name @call.external
  (call_suffix)) @call
  (#any-of? @call.name
    "print" "debugPrint" "fatalError" "precondition" "preconditionFailure" "assert"
    "assertionFailure" "min" "max" "abs" "zip" "stride" "type" "withExtendedLifetime"
    "String" "Int" "Double" "Float" "Bool" "Array" "Dictionary" "Set"))

(function_declaration
  body: (function_body) @body)
(init_declaration
  body: (function_body) @body)

(comment) @comment
(multiline_comment) @comment

(import_declaration
  (identifier) @import.path)
