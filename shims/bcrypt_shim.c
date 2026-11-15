/*
 * bcryptprimitives.dll shim for Windows 7 / Server 2008 R2.
 * ProcessPrng is redirected to RtlGenRandom (SystemFunction036 in advapi32).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#pragma comment(lib, "advapi32.lib")

/* RtlGenRandom is exported from advapi32.dll as SystemFunction036 */
BOOLEAN WINAPI SystemFunction036(PVOID RandomBuffer, ULONG RandomBufferLength);

__declspec(dllexport)
BOOL WINAPI ProcessPrng(PBYTE pbData, SIZE_T cbData) {
    return (BOOL)SystemFunction036(pbData, (ULONG)cbData);
}
