/* SPDX-License-Identifier: MPL-2.0
 *
 * The runtime service table (plcc_services.h, docs/program-image.md): the
 * address of every service, in index order. Each service is declared under a
 * private C name bound to the real symbol with an asm label, so the C
 * library's own prototypes (and GCC's built-in knowledge of memcpy, sinf, ...)
 * do not get in the way: only the address is taken here.
 */
#include "plcc_image.h"
#include "plcc_services.h"

#define DECLARE(index, name) extern void plcc_svc_##name(void) __asm__(#name);
PLCC_SERVICES(DECLARE)
#undef DECLARE

#define ENTRY(index, name) [index] = (const void *)plcc_svc_##name,
const plcc_service_table_t plcc_service_table = {
    PLCC_SERVICE_TABLE_MAGIC,
    PLCC_SERVICE_COUNT,
    {PLCC_SERVICES(ENTRY)},
};
#undef ENTRY
