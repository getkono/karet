/**
 * @file External scanner for Sass indented syntax
 *
 * This scanner handles:
 * - Indentation tracking (INDENT/DEDENT tokens)
 * - Newline detection
 * - String content parsing with interpolation support
 */

#include "tree_sitter/alloc.h"
#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <string.h>
#include <wctype.h>

enum TokenType { INDENT, DEDENT, NEWLINE, STRING_CONTENT, ERROR_SENTINEL };

#define MAX_INDENT_DEPTH 100

typedef struct {
  uint16_t indent_stack[MAX_INDENT_DEPTH];
  uint8_t stack_size;
  uint8_t dedent_count;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

void *tree_sitter_sass_external_scanner_create() {
  Scanner *scanner = ts_calloc(1, sizeof(Scanner));
  scanner->stack_size = 0;
  scanner->dedent_count = 0;
  scanner->indent_stack[0] = 0;
  return scanner;
}

void tree_sitter_sass_external_scanner_destroy(void *payload) {
  ts_free(payload);
}

unsigned tree_sitter_sass_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  Scanner *scanner = payload;
  unsigned size = 0;

  buffer[size++] = scanner->stack_size;
  buffer[size++] = scanner->dedent_count;

  for (uint8_t i = 0; i < scanner->stack_size &&
                      size < TREE_SITTER_SERIALIZATION_BUFFER_SIZE - 2;
       i++) {
    buffer[size++] = scanner->indent_stack[i] & 0xFF;
    buffer[size++] = (scanner->indent_stack[i] >> 8) & 0xFF;
  }

  return size;
}

void tree_sitter_sass_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  Scanner *scanner = payload;
  scanner->stack_size = 0;
  scanner->dedent_count = 0;

  if (length == 0) {
    scanner->indent_stack[0] = 0;
    return;
  }

  unsigned i = 0;
  scanner->stack_size = buffer[i++];
  if (i >= length)
    return;
  scanner->dedent_count = buffer[i++];

  for (uint8_t j = 0; j < scanner->stack_size && i + 1 < length; j++) {
    scanner->indent_stack[j] =
        (unsigned char)buffer[i] | ((unsigned char)buffer[i + 1] << 8);
    i += 2;
  }
}

static uint16_t current_indent(Scanner *scanner) {
  if (scanner->stack_size == 0)
    return 0;
  return scanner->indent_stack[scanner->stack_size - 1];
}

bool tree_sitter_sass_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  Scanner *scanner = payload;

  if (valid_symbols[ERROR_SENTINEL]) {
    return false;
  }

  if (valid_symbols[DEDENT] && scanner->dedent_count > 0) {
    scanner->dedent_count--;
    if (scanner->stack_size > 0)
      scanner->stack_size--;
    lexer->result_symbol = DEDENT;
    return true;
  }

  if (valid_symbols[STRING_CONTENT]) {
    bool has_content = false;

    while (!lexer->eof(lexer)) {
      if (lexer->lookahead == '"' || lexer->lookahead == '\'') {
        lexer->mark_end(lexer);
        lexer->result_symbol = STRING_CONTENT;
        return has_content;
      }

      if (lexer->lookahead == '\\') {
        lexer->mark_end(lexer);
        lexer->result_symbol = STRING_CONTENT;
        return has_content;
      }

      if (lexer->lookahead == '#') {
        lexer->mark_end(lexer);
        advance(lexer);
        if (lexer->lookahead == '{') {
          lexer->result_symbol = STRING_CONTENT;
          return has_content;
        }
        has_content = true;
        continue;
      }

      if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
        return false;
      }

      advance(lexer);
      has_content = true;
    }

    return false;
  }

  bool found_newline = false;
  uint16_t indent = 0;

  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    skip(lexer);
  }

  while (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
    found_newline = true;
    skip(lexer);
    indent = 0;

    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      if (lexer->lookahead == ' ') {
        indent++;
      } else {
        indent += 2;
      }
      skip(lexer);
    }

    while (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
      skip(lexer);
      indent = 0;
      while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        if (lexer->lookahead == ' ') {
          indent++;
        } else {
          indent += 2;
        }
        skip(lexer);
      }
    }
  }

  if (valid_symbols[NEWLINE] && found_newline) {
    lexer->mark_end(lexer);
    lexer->result_symbol = NEWLINE;

    uint16_t current = current_indent(scanner);

    if (lexer->eof(lexer)) {
      scanner->dedent_count = scanner->stack_size;
    }

    return true;
  }

  if (valid_symbols[INDENT]) {
    uint16_t current = current_indent(scanner);

    if (lexer->get_column(lexer) == 0) {
      indent = 0;
      while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        if (lexer->lookahead == ' ') {
          indent++;
        } else {
          indent += 2;
        }
        skip(lexer);
      }
    } else {
      indent = lexer->get_column(lexer);
    }

    if (indent > current && !lexer->eof(lexer) && lexer->lookahead != '\n' &&
        lexer->lookahead != '\r') {
      if (scanner->stack_size < MAX_INDENT_DEPTH) {
        scanner->indent_stack[scanner->stack_size++] = indent;
      }
      lexer->mark_end(lexer);
      lexer->result_symbol = INDENT;
      return true;
    }
  }

  if (valid_symbols[DEDENT]) {
    uint16_t col = lexer->get_column(lexer);
    uint16_t current = current_indent(scanner);

    if (col < current && scanner->stack_size > 0) {
      uint8_t dedents = 0;
      for (int i = scanner->stack_size - 1; i >= 0; i--) {
        if (scanner->indent_stack[i] > col) {
          dedents++;
        } else {
          break;
        }
      }

      if (dedents > 0) {
        scanner->dedent_count = dedents - 1;
        scanner->stack_size--;
        lexer->result_symbol = DEDENT;
        return true;
      }
    }
  }

  return false;
}
