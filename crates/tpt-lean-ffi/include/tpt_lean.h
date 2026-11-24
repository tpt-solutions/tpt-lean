/* SPDX-License-Identifier: MIT OR Apache-2.0
 * tpt_lean.h: C ABI for tpt-lean. ABI version 1.
 *
 * Ownership: strings returned in out parameters are owned by the caller and must be freed
 * with tptl_string_free. Handles must be freed with their matching *_free function.
 * Thread safety: TptlContext is thread-safe; TptlIndex calls are serialized internally;
 * TptlSearchResults is immutable after creation.
 */
#ifndef TPT_LEAN_H
#define TPT_LEAN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TptlContext TptlContext;
typedef struct TptlIndex TptlIndex;
typedef struct TptlSearchResults TptlSearchResults;

typedef enum TptlStatus {
  TPTL_OK = 0,
  TPTL_ERR_INVALID_ARGUMENT = 1,
  TPTL_ERR_IO = 2,
  TPTL_ERR_LEAN_NOT_FOUND = 3,
  TPTL_ERR_INDEX = 4,
  TPTL_ERR_INTERNAL = 5
} TptlStatus;

uint32_t tptl_abi_version(void);

TptlStatus tptl_context_new(TptlContext** out_ctx);
void tptl_context_free(TptlContext* ctx);
/* Valid until the next call on the same context. Never NULL. */
const char* tptl_last_error(TptlContext* ctx);

TptlStatus tptl_index_open(TptlContext* ctx, const char* project_path, TptlIndex** out_index);
void tptl_index_free(TptlIndex* index);

/* limit 0 means 10. */
TptlStatus tptl_search_query(TptlIndex* index, const char* query, uint32_t limit,
                             TptlSearchResults** out_results);
/* Valid until tptl_search_results_free. */
const char* tptl_search_results_to_json(const TptlSearchResults* results);
void tptl_search_results_free(TptlSearchResults* results);

/* On success *out_formatted is owned by the caller: free with tptl_string_free. */
TptlStatus tptl_format_text(TptlContext* ctx, const char* input, char** out_formatted);
void tptl_string_free(char* value);

#ifdef __cplusplus
}
#endif

#endif /* TPT_LEAN_H */
