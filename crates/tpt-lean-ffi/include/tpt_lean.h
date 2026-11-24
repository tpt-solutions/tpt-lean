/* SPDX-License-Identifier: MIT OR Apache-2.0
 * tpt_lean.h: C ABI for tpt-lean. ABI version 1.
 *
 * Ownership: strings returned in out parameters are owned by the caller and must be freed
 * with tptl_string_free. Handles must be freed with their matching *_free function. Null
 * handles passed to a *_free function are ignored.
 *
 * Errors: functions that take a TptlContext leave a message on it (see tptl_last_error).
 * tptl_search_query takes no context, so its failures are reported by status code only.
 *
 * Thread safety (see docs/ffi.md):
 *   TptlContext       thread-safe
 *   TptlIndex         thread-safe; calls are serialized internally (concurrent reads are safe)
 *   TptlSearchResults immutable after creation; safe to read from any thread
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

/* ABI version of this library (separate from the crate version). Returns 0 only if an
 * internal panic occurred. */
uint32_t tptl_abi_version(void);

/* Create a context. Free with tptl_context_free. */
TptlStatus tptl_context_new(TptlContext** out_ctx);
void tptl_context_free(TptlContext* ctx);
/* Last error message for ctx, or "". Never NULL. Valid until the next call on the same
 * context. Passing NULL returns "". */
const char* tptl_last_error(TptlContext* ctx);

/* Open the index of the Lean project containing project_path (a file or directory).
 * Free with tptl_index_free. */
TptlStatus tptl_index_open(TptlContext* ctx, const char* project_path, TptlIndex** out_index);
void tptl_index_free(TptlIndex* index);

/* limit 0 means 10; values above 1000 are clamped to 1000. Free results with
 * tptl_search_results_free. */
TptlStatus tptl_search_query(TptlIndex* index, const char* query, uint32_t limit,
                             TptlSearchResults** out_results);
/* JSON text of the results, e.g. {"items":[{"name":..,"kind":..,"package":..,"source":..,
 * "line":..,"score":..}]}. Valid until tptl_search_results_free. NULL if results is NULL. */
const char* tptl_search_results_to_json(const TptlSearchResults* results);
void tptl_search_results_free(TptlSearchResults* results);

/* Format Lean source. On success *out_formatted is owned by the caller: free with
 * tptl_string_free. Returns TPTL_ERR_INVALID_ARGUMENT when the input cannot be formatted
 * safely (uncertain syntax); nothing is written in that case. */
TptlStatus tptl_format_text(TptlContext* ctx, const char* input, char** out_formatted);

/* Parse Lean source. On success *out_json is a JSON object owned by the caller (free with
 * tptl_string_free) with keys: source_len, tokens [{kind,start,end}], imports
 * [{module,line,start,end}], symbols [{name,short_name,kind,namespace,line,docstring,
 * is_private,start,end}], uncertain [{reason,start,end}]. Offsets are UTF-8 byte offsets. */
TptlStatus tptl_syntax_parse(TptlContext* ctx, const char* source, char** out_json);

/* Parse Lean CLI output text. On success *out_json is {"diagnostics":[{file,line,column,
 * severity,message}]} owned by the caller (free with tptl_string_free). Unrecognised lines
 * are ignored; no diagnostics yields an empty array. */
TptlStatus tptl_diagnostics_parse(TptlContext* ctx, const char* lean_output, char** out_json);

/* Look up a cache entry by digest (64 lowercase hex characters) in the global cache
 * directory. On success *out_json is {"digest","found","corrupt","entry"} owned by the
 * caller (free with tptl_string_free). A missing entry is found=false, not an error. */
TptlStatus tptl_cache_query(TptlContext* ctx, const char* digest, char** out_json);

/* Free a string returned by this library. NULL is accepted. */
void tptl_string_free(char* value);

#ifdef __cplusplus
}
#endif

#endif /* TPT_LEAN_H */
