/*
 * api-ms-win-core-synch-l1-2-0.dll shim for Windows 7 / Server 2008 R2.
 * Implements WaitOnAddress / WakeByAddress* using SRWLOCK + CONDITION_VARIABLE
 * (both available since Vista/2008 R2).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

static SRWLOCK            s_lock = SRWLOCK_INIT;
static CONDITION_VARIABLE s_cond = CONDITION_VARIABLE_INIT;

static BOOL _eq(volatile VOID *addr, PVOID cmp, SIZE_T sz) {
    switch (sz) {
        case 1: return (*(volatile UINT8  *)addr == *(UINT8  *)cmp);
        case 2: return (*(volatile UINT16 *)addr == *(UINT16 *)cmp);
        case 4: return (*(volatile UINT32 *)addr == *(UINT32 *)cmp);
        case 8: return (*(volatile UINT64 *)addr == *(UINT64 *)cmp);
        default: return FALSE;
    }
}

BOOL WINAPI WaitOnAddress(volatile VOID *Address, PVOID CompareAddress,
                          SIZE_T AddressSize, DWORD dwMilliseconds) {
    AcquireSRWLockShared(&s_lock);
    if (!_eq(Address, CompareAddress, AddressSize)) {
        ReleaseSRWLockShared(&s_lock);
        return TRUE;
    }
    DWORD timeout = (dwMilliseconds == 0) ? INFINITE : dwMilliseconds;
    BOOL ok = SleepConditionVariableSRW(&s_cond, &s_lock, timeout,
                                        CONDITION_VARIABLE_LOCKMODE_SHARED);
    DWORD err = GetLastError();
    ReleaseSRWLockShared(&s_lock);
    if (!ok) SetLastError(err);
    return ok;
}

void WINAPI WakeByAddressSingle(PVOID Address) {
    (void)Address;
    WakeConditionVariable(&s_cond);
}

void WINAPI WakeByAddressAll(PVOID Address) {
    (void)Address;
    WakeAllConditionVariable(&s_cond);
}
