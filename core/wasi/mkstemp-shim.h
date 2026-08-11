/* SPDX-License-Identifier: LicenseRef-BSL-1.1
 *
 * WASI's libc has no mkstemp, and EPANET wants one for its scratch files. A counter is enough
 * here: the module owns its whole filesystem, which the host creates empty for every run.
 */
#pragma once
#include <fcntl.h>
#include <string.h>

static int trama_temporary_counter = 0;

static int mkstemp(char *pattern) {
    size_t length = strlen(pattern);
    size_t at = length >= 6 ? length - 6 : 0;
    int serial = trama_temporary_counter++;
    for (size_t index = length; index-- > at;) {
        pattern[index] = (char)('a' + (serial % 26));
        serial /= 26;
    }
    return open(pattern, O_RDWR | O_CREAT | O_EXCL, 0600);
}
