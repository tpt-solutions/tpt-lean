/* SPDX-License-Identifier: MIT OR Apache-2.0
 * C harness for the tpt-lean ABI. Compiles against include/tpt_lean.h and links the shared
 * library built by `cargo build -p tpt-lean-ffi`. Exits 0 on success, 1 on the first failure.
 */
#include <stdio.h>
#include <string.h>

#include "tpt_lean.h"

static int failures = 0;

#define CHECK(cond, msg)                                              \
  do {                                                                \
    if (!(cond)) {                                                    \
      fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, (msg)); \
      failures++;                                                     \
    }                                                                 \
  } while (0)

static void test_abi_version(void) {
  CHECK(tptl_abi_version() == 1u, "ABI version must be 1");
}

static void test_format_round_trip(TptlContext* ctx) {
  char* out = NULL;
  TptlStatus st = tptl_format_text(ctx, "def x := 1   \n", &out);
  CHECK(st == TPTL_OK, "format_text should succeed");
  CHECK(out != NULL && strcmp(out, "def x := 1\n") == 0, "formatted text mismatch");
  tptl_string_free(out);
}

static void test_error_text(TptlContext* ctx) {
  char* out = NULL;
  TptlStatus st = tptl_format_text(ctx, NULL, &out);
  CHECK(st == TPTL_ERR_INVALID_ARGUMENT, "NULL input must be TPTL_ERR_INVALID_ARGUMENT");
  CHECK(out == NULL, "out must stay NULL on error");
  CHECK(strstr(tptl_last_error(ctx), "input is null") != NULL, "error text mismatch");
}

static void test_null_free(void) {
  tptl_string_free(NULL);
  tptl_index_free(NULL);
  tptl_search_results_free(NULL);
  tptl_context_free(NULL);
}

static void test_syntax_and_diagnostics(TptlContext* ctx) {
  char* out = NULL;
  TptlStatus st = tptl_syntax_parse(ctx, "import Mathlib.Logic\n", &out);
  CHECK(st == TPTL_OK && out != NULL && strstr(out, "Mathlib.Logic") != NULL,
        "syntax_parse should report the import");
  tptl_string_free(out);

  out = NULL;
  st = tptl_diagnostics_parse(ctx, "Main.lean:1:2: error: boom\n", &out);
  CHECK(st == TPTL_OK && out != NULL && strstr(out, "\"severity\":\"error\"") != NULL,
        "diagnostics_parse should report the error");
  tptl_string_free(out);
}

static void test_cache_query_rejects_bad_digest(TptlContext* ctx) {
  char* out = NULL;
  TptlStatus st = tptl_cache_query(ctx, "../etc", &out);
  CHECK(st == TPTL_ERR_INVALID_ARGUMENT && out == NULL, "bad digest must be rejected");
}

int main(void) {
  TptlContext* ctx = NULL;
  test_abi_version();
  if (tptl_context_new(&ctx) != TPTL_OK || ctx == NULL) {
    fprintf(stderr, "FAIL: tptl_context_new\n");
    return 1;
  }
  test_format_round_trip(ctx);
  test_error_text(ctx);
  test_null_free();
  test_syntax_and_diagnostics(ctx);
  test_cache_query_rejects_bad_digest(ctx);
  tptl_context_free(ctx);

  if (failures == 0) {
    printf("tpt-lean C harness: all checks passed\n");
    return 0;
  }
  fprintf(stderr, "tpt-lean C harness: %d failure(s)\n", failures);
  return 1;
}
