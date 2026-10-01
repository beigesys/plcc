/* SPDX-License-Identifier: MPL-2.0
 *
 * plcc program images (docs/program-image.md): the header, the runtime
 * contract types (ABI 1, docs/process-image.md) and the checks a loader runs
 * before it touches a program. Plain C99, no board code: the Opta runtime
 * (loader.ino) and the emulator harness (crates/plcc-image/tests/harness)
 * compile the same file.
 */
#ifndef PLCC_IMAGE_H
#define PLCC_IMAGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PLCC_IMAGE_MAGIC 0x49434C50u         /* "PLCI" */
#define PLCC_IMAGE_FORMAT 1u
#define PLCC_IMAGE_HEADER_SIZE 128u
#define PLCC_SERVICE_TABLE_MAGIC 0x53434C50u /* "PLCS" */
#define PLCC_IMAGE_RAM_RESERVED 8u           /* services pointer + 4 spare bytes at the window start */

/* Little-endian, at the first byte of the program slot. */
typedef struct plcc_image_header {
  uint32_t magic;          /*   0  PLCC_IMAGE_MAGIC */
  uint16_t format;         /*   4  PLCC_IMAGE_FORMAT */
  uint16_t header_size;    /*   6  PLCC_IMAGE_HEADER_SIZE */
  uint32_t image_size;     /*   8  header + body, bytes (a multiple of 32) */
  uint32_t body_crc32;     /*  12  CRC-32 of bytes [header_size, image_size) */
  char target_id[24];      /*  16  device manifest id, NUL-padded ("arduino-opta") */
  uint32_t target_version; /*  40  device manifest version it was linked for */
  uint32_t abi;            /*  44  runtime-contract ABI (PLCC_ABI_VERSION) */
  uint32_t services;       /*  48  service-table entries it uses (highest index + 1) */
  uint32_t slot_addr;      /*  52  flash address it was linked at */
  uint32_t slot_size;      /*  56  size of that slot */
  uint32_t ram_addr;       /*  60  RAM window it was linked for */
  uint32_t ram_size;       /*  64 */
  uint32_t text_addr;      /*  68  code, read-only data and veneers: execute in place */
  uint32_t text_size;      /*  72 */
  uint32_t data_load;      /*  76  flash address of the .data initial values */
  uint32_t data_addr;      /*  80  RAM address .data is copied to */
  uint32_t data_size;      /*  84 */
  uint32_t bss_addr;       /*  88  RAM zeroed before init */
  uint32_t bss_size;       /*  92 */
  uint32_t services_slot;  /*  96  RAM word the loader stores the service table's address in */
  uint32_t get_app;        /* 100  plcc_get_app (Thumb: bit 0 set) */
  uint32_t flags;          /* 104  0 */
  uint8_t build_id[16];    /* 108  identifies the build (hash of the object or the sources) */
  uint32_t header_crc32;   /* 124  CRC-32 of bytes [0, 124) */
} plcc_image_header_t;

typedef char plcc_image_header_is_128_bytes[sizeof(plcc_image_header_t) == PLCC_IMAGE_HEADER_SIZE ? 1 : -1];

/* The service table (plcc_services.h): a program's veneers load fn[index]. */
typedef struct plcc_service_table {
  uint32_t magic; /* PLCC_SERVICE_TABLE_MAGIC */
  uint32_t count;
  const void *fn[];
} plcc_service_table_t;

/* ── The runtime contract, ABI 1 (docs/process-image.md) ───────────── */
typedef struct plcc_process_image {
  uint8_t *input;
  uint32_t input_size;
  uint8_t *output;
  uint32_t output_size;
  uint8_t *memory;
  uint32_t memory_size;
} plcc_process_image_t;

typedef struct plcc_program_instance {
  const char *name;
  const char *program_type;
  void (*init)(void *state);
  void (*scan)(void *state);
  void *state;
  uint64_t state_size;
} plcc_program_instance_t;

typedef struct plcc_task {
  const char *name;
  int64_t interval_ns;
  uint32_t priority;
  uint32_t program_count;
  uint8_t (*single)(void);
  const plcc_program_instance_t *programs;
} plcc_task_t;

typedef struct plcc_retain_region {
  const char *name;
  void *data;
  uint64_t size;
} plcc_retain_region_t;

typedef struct plcc_app {
  uint32_t abi_version;
  uint32_t task_count;
  const plcc_task_t *tasks;
  const plcc_process_image_t *image;
  void (*init)(void);
  void (*run_task)(uint32_t task);
  const plcc_retain_region_t *retain;
  uint32_t retain_count;
  uint32_t retain_signature;
} plcc_app_t;

#define PLCC_FAULT_DIV_BY_ZERO 1u
#define PLCC_FAULT_ARRAY_BOUNDS 2u
#define PLCC_FAULT_NULL_REFERENCE 3u
#define PLCC_FAULT_USER_BASE 0x10000u
/* Faults the loader raises itself. */
#define PLCC_FAULT_CPU (PLCC_FAULT_USER_BASE + 1)      /* HardFault/BusFault/... inside the program */
#define PLCC_FAULT_WATCHDOG (PLCC_FAULT_USER_BASE + 2) /* a scan ran longer than the watchdog */
#define PLCC_MAX_TASKS 32u

/* What this runtime accepts. */
typedef struct plcc_image_expect {
  const char *target_id;
  uint32_t target_version_min, target_version_max;
  uint32_t abi;
  uint32_t slot_addr, slot_size;
  uint32_t ram_addr, ram_size;
  uint32_t services; /* entries in this runtime's table */
  uint32_t image_i, image_q, image_m; /* process-image sizes the runtime's I/O is built for */
} plcc_image_expect_t;

uint32_t plcc_crc32(uint32_t crc, const void *data, size_t len);

/* Validate the image in the slot without executing anything in it. Returns 1
 * if valid; otherwise 0 and *why says what is wrong ("empty slot" for an
 * erased one). Reads only [slot_addr, slot_addr + slot_size). */
int plcc_image_check(const plcc_image_expect_t *x, const char **why);

/* For a checked image: clear the RAM window, copy .data, store `table` in the
 * services slot. Returns the program's plcc_get_app. */
typedef const plcc_app_t *(*plcc_get_app_fn)(void);
plcc_get_app_fn plcc_image_prepare(const plcc_image_expect_t *x, const plcc_service_table_t *table);

/* Check the descriptor plcc_get_app returned: every pointer inside the image
 * (code in its text, state in its RAM window), the ABI, the task table and the
 * process-image sizes. Returns 1 if usable. */
int plcc_image_check_app(const plcc_image_expect_t *x, const plcc_app_t *app, const char **why);

/* 1 if [p, p + len) lies inside the image's text (code/read-only data). */
int plcc_image_in_text(const plcc_image_expect_t *x, uintptr_t p, size_t len);
/* 1 if [p, p + len) lies inside the image's RAM window. */
int plcc_image_in_ram(const plcc_image_expect_t *x, uintptr_t p, size_t len);

#ifdef __cplusplus
}
#endif
#endif
