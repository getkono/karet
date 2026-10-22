; Calls, bodies, comments and imports for the seam call tier.
; Capture names: crates/karet-seam/src/calls/site.rs.

; f(), x.f(), self.f(), super().f()
(call
  function: (identifier) @call.name) @call
(call
  function: (attribute
    object: (_) @call.receiver
    attribute: (identifier) @call.name)) @call
((call
  function: (attribute
    object: (call
      function: (identifier) @_super) @call.super
    attribute: (identifier) @call.name)) @call
  (#eq? @_super "super"))

; Builtins are always someone else's.
((call
  function: (identifier) @call.name @call.external) @call
  (#any-of? @call.name
    "print" "len" "range" "isinstance" "issubclass" "str" "int" "float" "bool" "list"
    "dict" "set" "tuple" "frozenset" "bytes" "repr" "hash" "id" "type" "super" "open"
    "iter" "next" "enumerate" "zip" "map" "filter" "sorted" "reversed" "min" "max"
    "sum" "any" "all" "abs" "round" "getattr" "setattr" "hasattr" "delattr" "vars"
    "callable" "format" "ord" "chr" "object" "property" "staticmethod" "classmethod"))

(function_definition
  body: (block) @body)

(comment) @comment
; A docstring is documentation written as an expression.
(module . (expression_statement (string) @comment))
(function_definition
  body: (block . (expression_statement (string) @comment)))
(class_definition
  body: (block . (expression_statement (string) @comment)))

; `import a.b` binds `a`, not `b`.
(import_statement
  name: (dotted_name) @import.path @import.head)
(import_statement
  name: (aliased_import
    name: (dotted_name) @import.path
    alias: (identifier) @import.alias))
(import_from_statement
  module_name: (_) @import.prefix
  name: (dotted_name) @import.path)
(import_from_statement
  module_name: (_) @import.prefix
  name: (aliased_import
    name: (dotted_name) @import.path
    alias: (identifier) @import.alias))
(import_from_statement
  module_name: (_) @import.glob
  (wildcard_import))
