/*
 * LightCraft's engine as the SwiftUI iOS app sees it: the C ABI of the Rust static library
 * `liblightcraft_bridge.a` (apps/lightcraft-ios-swift/src/abi.rs; docs/ios-swift.md → The C ABI).
 *
 * Written by hand, and checked against the Rust code by the test `header_matches_exports`
 * (names, parameters, LC_ABI_VERSION, status codes, struct layout): change both together.
 *
 * - Messages are UTF-8 JSON. Requests are {"id", "method", "params"}; lc_poll returns a JSON array
 *   of events ({"kind": "reply" | "image" | "state" | ...}).
 * - Engines are named by a handle (a number, never a pointer): a stale or unknown handle is
 *   LC_BAD_HANDLE, never undefined behaviour.
 * - Every buffer returned (LcBuf, LcImage) is a lease: valid until lc_release(token), then gone.
 *   Release each exactly once (releasing an unknown token does nothing).
 * - A panic inside never crosses into the caller: it comes back as LC_PANIC.
 * - Every function may be called from any thread; lc_poll blocks (up to its timeout), so call it
 *   from a thread of its own.
 */
#pragma once

#include <stddef.h>
#include <stdint.h>

#define LC_ABI_VERSION 1u

/* status codes */
#define LC_OK 0
#define LC_ERROR 1
#define LC_BAD_HANDLE 2
#define LC_BAD_INPUT 3
#define LC_PANIC 4
#define LC_CLOSED 5
#define LC_GONE 6

/* JSON bytes owned by the library until lc_release(token). When status != LC_OK they are an error
   object: {"code": "...", "message": "..."}. */
typedef struct LcBuf {
    const uint8_t *ptr;
    size_t len;
    uint64_t token;
    int32_t status;
} LcBuf;

/* The pixels of an `image` event: RGBA8, sRGB, straight alpha, `stride` bytes per row, owned by
   the library until lc_release of the event's token. pixels is NULL unless status == LC_OK. */
typedef struct LcImage {
    const uint8_t *pixels;
    size_t len;
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    int32_t status;
} LcImage;

_Static_assert(sizeof(LcBuf) == 32, "LcBuf layout");
_Static_assert(sizeof(LcImage) == 32, "LcImage layout");

/* The ABI this library speaks (LC_ABI_VERSION when header and library match). */
uint32_t lc_abi_version(void);

/* Start an engine. config: JSON {home?, tmp?, library?, demo?, deviceName?, script?,
   renderThreads?, availableMemory?}. Answer: {"engine": handle, "library", "photos", "demo",
   "warnings"}. Opening the library can take a moment: not on the main thread. */
LcBuf lc_open(const uint8_t *config, size_t len);

/* Save, stop the engine's threads (waiting a few seconds at most) and forget the handle. */
void lc_close(uint64_t engine);

/* Queue a request; its answer comes as a `reply` event from lc_poll. LC_OK, LC_BAD_HANDLE,
   LC_BAD_INPUT (not a request), LC_CLOSED or LC_PANIC. */
int32_t lc_submit(uint64_t engine, const uint8_t *request, size_t len);

/* Run a request and wait up to timeout_ms for its `reply` event (start-up, tests). */
LcBuf lc_call(uint64_t engine, const uint8_t *request, size_t len, uint32_t timeout_ms);

/* The events since the last poll (a JSON array, possibly empty), waiting up to timeout_ms for one. */
LcBuf lc_poll(uint64_t engine, uint32_t timeout_ms);

/* A waiting lc_poll returns now (to stop a polling thread). */
void lc_wake(uint64_t engine);

/* The pixels of an `image` event's token. LC_GONE when they were dropped before being taken (a
   newer picture of the same slot came, or the app fell far behind): wait for the next one. */
LcImage lc_image(uint64_t token);

/* Free a buffer of lc_open, lc_call, lc_poll or lc_image (by its token). */
void lc_release(uint64_t token);
