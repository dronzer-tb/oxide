package dev.oxide.plugin.pacside;

import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Foreign Function & Memory API (Panama FFI) bindings for the native Pacside packet cache.
 */
public final class PacsideNative {

    private static final Logger LOGGER = Logger.getLogger(PacsideNative.class.getName());
    private static volatile boolean available = false;

    private static MethodHandle pacsideInitHandle;
    private static MethodHandle pacsidePutHandle;
    private static MethodHandle pacsideGetHandle;
    private static MethodHandle pacsideInvalidateHandle;
    private static MethodHandle pacsideClearHandle;

    public static synchronized void init(SymbolLookup lookup) {
        if (available) return;
        Linker linker = Linker.nativeLinker();

        try {
            MemorySegment initSym = lookup.find("pacside_ffi_init").orElse(null);
            MemorySegment putSym = lookup.find("pacside_ffi_put").orElse(null);
            MemorySegment getSym = lookup.find("pacside_ffi_get").orElse(null);
            MemorySegment invalidateSym = lookup.find("pacside_ffi_invalidate").orElse(null);
            MemorySegment clearSym = lookup.find("pacside_ffi_clear").orElse(null);

            if (initSym != null && putSym != null && getSym != null) {
                pacsideInitHandle = linker.downcallHandle(initSym,
                        FunctionDescriptor.ofVoid(ValueLayout.JAVA_LONG));
                pacsidePutHandle = linker.downcallHandle(putSym,
                        FunctionDescriptor.of(ValueLayout.JAVA_INT,
                                ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT, ValueLayout.JAVA_INT,
                                ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
                pacsideGetHandle = linker.downcallHandle(getSym,
                        FunctionDescriptor.of(ValueLayout.JAVA_INT,
                                ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT, ValueLayout.JAVA_INT,
                                ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));

                if (invalidateSym != null) {
                    pacsideInvalidateHandle = linker.downcallHandle(invalidateSym,
                            FunctionDescriptor.ofVoid(ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT, ValueLayout.JAVA_INT));
                }
                if (clearSym != null) {
                    pacsideClearHandle = linker.downcallHandle(clearSym, FunctionDescriptor.ofVoid());
                }

                available = true;
                // Initialize with 32,768 chunks capacity (~1GB RAM)
                pacsideInitHandle.invokeExact(32768L);
                LOGGER.info("[Pacside] Native off-heap packet cache initialized successfully via Panama FFI.");
            }
        } catch (Throwable e) {
            LOGGER.log(Level.WARNING, "[Pacside] Could not bind native pacside FFI symbols", e);
        }
    }

    public static boolean isAvailable() {
        return available;
    }

    public static void put(long worldId, int chunkX, int chunkZ, MemorySegment buffer, long length) {
        if (!available || pacsidePutHandle == null) return;
        try {
            pacsidePutHandle.invokeExact(worldId, chunkX, chunkZ, buffer, length);
        } catch (Throwable t) {
            LOGGER.log(Level.FINE, "[Pacside] Native put error", t);
        }
    }

    public static int get(long worldId, int chunkX, int chunkZ, MemorySegment outBuffer, long maxLength) {
        if (!available || pacsideGetHandle == null) return 0;
        try {
            return (int) pacsideGetHandle.invokeExact(worldId, chunkX, chunkZ, outBuffer, maxLength);
        } catch (Throwable t) {
            LOGGER.log(Level.FINE, "[Pacside] Native get error", t);
            return 0;
        }
    }

    public static void invalidate(long worldId, int chunkX, int chunkZ) {
        if (!available || pacsideInvalidateHandle == null) return;
        try {
            pacsideInvalidateHandle.invokeExact(worldId, chunkX, chunkZ);
        } catch (Throwable ignored) {}
    }

    public static void clear() {
        if (!available || pacsideClearHandle == null) return;
        try {
            pacsideClearHandle.invokeExact();
        } catch (Throwable ignored) {}
    }
}
