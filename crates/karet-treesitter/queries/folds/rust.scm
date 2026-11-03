; karet fold query for Rust. `@fold`: a construct that folds from the line it
; starts on through the line it ends on. `@fold.comment` / `@fold.imports`: one
; comment / import declaration; a run of them on adjacent lines folds as one.

[
  (function_item)
  (function_signature_item)
  (impl_item)
  (trait_item)
  (struct_item)
  (enum_item)
  (union_item)
  (mod_item)
  (foreign_mod_item)
  (macro_definition)
  (macro_invocation)
  (const_item)
  (static_item)
  (block)
  (match_block)
  (match_arm)
  (closure_expression)
  (struct_expression)
  (field_initializer_list)
  (field_declaration_list)
  (ordered_field_declaration_list)
  (enum_variant_list)
  (declaration_list)
  (arguments)
  (parameters)
  (array_expression)
  (tuple_expression)
  (use_list)
  (token_tree)
  (raw_string_literal)
  (string_literal)
] @fold

[
  (line_comment)
  (block_comment)
] @fold.comment

(use_declaration) @fold.imports
