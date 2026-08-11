# SPDX-License-Identifier: LicenseRef-BSL-1.1
# Builds EPANET's C against the WASI sysroot. WASI_SDK points at an unpacked wasi-sdk.
set(CMAKE_SYSTEM_NAME WASI)
set(CMAKE_SYSTEM_VERSION 1)
set(CMAKE_SYSTEM_PROCESSOR wasm32)
set(CMAKE_C_COMPILER $ENV{WASI_SDK}/bin/clang)
set(CMAKE_CXX_COMPILER $ENV{WASI_SDK}/bin/clang++)
set(CMAKE_AR $ENV{WASI_SDK}/bin/llvm-ar)
set(CMAKE_RANLIB $ENV{WASI_SDK}/bin/llvm-ranlib)
set(CMAKE_SYSROOT $ENV{WASI_SDK}/share/wasi-sysroot)
# CMake's compiler probe links an executable, which this target cannot do on its own.
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
# The shim belongs here rather than in CFLAGS: zstd builds an assembly file that a C header
# cannot be included into, and CFLAGS would reach it too.
set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -include ${CMAKE_CURRENT_LIST_DIR}/mkstemp-shim.h")
