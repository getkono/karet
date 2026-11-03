; karet fold query for JSON (and JSONC). `@fold`: an object or array folds from
; the line it opens on through the line it closes on. `@fold.comment`: one
; comment; a run of them on adjacent lines folds as one.

[
  (object)
  (array)
] @fold

(comment) @fold.comment
