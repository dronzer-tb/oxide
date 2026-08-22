package dev.oxide.plugin.ffi;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.file.Path;

/**
 * Panama (java.lang.foreign) bindings for {@code oxide-ffi}'s C ABI — see
 * {@code crates/oxide-ffi/src/lib.rs} for the Rust side of every method here;
 * signatures must match that file exactly, field for field.
 *
 * <p>Method names/signatures below (Arena.allocateFrom(String),
 * MemorySegment.getString(long), Linker.downcallHandle) were confirmed
 * against a real JDK 25 run in the session that wrote this class — see
 * docs/REFERENCE_DATA.md — not guessed from an older preview-era API.
 *
 * <p><b>Requires {@code --enable-native-access=ALL-UNNAMED}</b> (or the
 * module-qualified form, if Paper ever loads plugins as named modules) on the
 * server's JVM command line — Panama's downcalls are a restricted operation
 * and JDK 25 only warns today, but a future JDK release blocks them outright
 * without this flag. Not verified against an actual running Folia process in
 * this session (no Folia server available to launch here) — if plugin load
 * fails with an {@code IllegalCallerException} on a real server, this flag is
 * the first thing to check.
 *
 * <p>Single-platform scope cut: loads exactly the library at the path given
 * to the constructor and does no per-OS/arch resolution of its own. The jar
 * does embed one prebuilt linux-x86_64 library, which
 * {@code GeneratorService#extractBundledLibrary} unpacks to a real filesystem
 * path before calling this constructor ({@code SymbolLookup.libraryLookup}
 * cannot dlopen a jar entry). Anything beyond that one platform is a separate,
 * real piece of work neither class attempts.
 */
public final class OxideNative implements AutoCloseable {

    private static final Linker LINKER = Linker.nativeLinker();
    /** Generous upper bound for reading a NUL-terminated string back out of a raw pointer. */
    private static final long MAX_C_STRING_LEN = 1 << 16;

    private final Arena arena;
    private final MethodHandle hOpen;
    private final MethodHandle hClose;
    private final MethodHandle hMinY;
    private final MethodHandle hHeight;
    private final MethodHandle hSeaLevel;
    private final MethodHandle hDefaultBlockName;
    private final MethodHandle hDefaultFluidName;
    private final MethodHandle hGenerateChunk;
    private final MethodHandle hLastError;

    public OxideNative(Path libraryPath) {
        this.arena = Arena.ofShared();
        SymbolLookup lookup = SymbolLookup.libraryLookup(libraryPath, arena);

        this.hOpen = downcall(lookup, "oxide_open", FunctionDescriptor.of(
                ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
        this.hClose = downcall(lookup, "oxide_close", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));
        this.hMinY = downcall(lookup, "oxide_min_y",
                FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        this.hHeight = downcall(lookup, "oxide_height",
                FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        this.hSeaLevel = downcall(lookup, "oxide_sea_level",
                FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS));
        this.hDefaultBlockName = downcall(lookup, "oxide_default_block_name",
                FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        this.hDefaultFluidName = downcall(lookup, "oxide_default_fluid_name",
                FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        this.hGenerateChunk = downcall(lookup, "oxide_generate_chunk", FunctionDescriptor.of(
                ValueLayout.JAVA_LONG, ValueLayout.ADDRESS, ValueLayout.JAVA_INT,
                ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
        this.hLastError = downcall(lookup, "oxide_last_error", FunctionDescriptor.of(ValueLayout.ADDRESS));
    }

    private static MethodHandle downcall(SymbolLookup lookup, String name, FunctionDescriptor descriptor) {
        MemorySegment symbol = lookup.find(name)
                .orElseThrow(() -> new IllegalStateException("native symbol not found: " + name
                        + " -- is the oxide-ffi library the one built from this repo's current source?"));
        return LINKER.downcallHandle(symbol, descriptor);
    }

    /** One open generator handle. Not thread-safe by itself — see {@link #generateChunk}. */
    public final class Handle implements AutoCloseable {
        private final MemorySegment ptr;
        private volatile boolean closed;

        private Handle(MemorySegment ptr) {
            this.ptr = ptr;
        }

        public int minY() {
            return invokeInt(hMinY, ptr);
        }

        public int height() {
            return invokeInt(hHeight, ptr);
        }

        public int seaLevel() {
            return invokeInt(hSeaLevel, ptr);
        }

        public String defaultBlockName() {
            return readCString(invokeAddress(hDefaultBlockName, ptr));
        }

        public String defaultFluidName() {
            return readCString(invokeAddress(hDefaultFluidName, ptr));
        }

        /**
         * Fills {@code out} (must be exactly {@code 256 * height()} bytes) with one byte per
         * block: {@code 0}/{@code 1}/{@code 2} for air/default block/default fluid, index
         * {@code (y*16+z)*16+x} with {@code y} relative to {@code minY()} across the whole
         * column (not restarted per 16-tall section) — see {@code oxide_generate_chunk}'s doc
         * in {@code oxide-ffi/src/lib.rs}.
         *
         * <p>Safe to call concurrently on the same handle from multiple threads — the Rust
         * generator holds no mutable state after {@code open()} (every field is read-only
         * settings/router data), so there's no data race to worry about, which matters since
         * Folia generates chunks in different regions on different threads. The one thing that
         * is <em>not</em> safe is racing this against {@link #close()} on the same handle: a
         * close while a generation call is still in flight on another thread is a use-after-free
         * on the Rust side (the raw pointer crossing the FFI boundary erases Rust's lifetime
         * tracking, so nothing catches this automatically) — callers must ensure every in-flight
         * {@code generateChunk} call has returned before closing.
         */
        public void generateChunk(int chunkX, int chunkZ, MemorySegment out) {
            long written;
            try {
                written = (long) hGenerateChunk.invokeExact(ptr, chunkX, chunkZ, out, out.byteSize());
            } catch (Throwable t) {
                throw new RuntimeException("oxide_generate_chunk threw across the FFI boundary", t);
            }
            if (written < 0) {
                throw new IllegalStateException(
                        "oxide_generate_chunk failed (code " + written + "): " + lastError());
            }
        }

        @Override
        public void close() {
            if (closed) {
                return;
            }
            closed = true;
            try {
                hClose.invokeExact(ptr);
            } catch (Throwable t) {
                throw new RuntimeException("oxide_close threw across the FFI boundary", t);
            }
        }
    }

    /**
     * Opens a generator for {@code dimensionId} (e.g. {@code "minecraft:overworld"}) within the
     * datapack at {@code datapackPath}, seeded with {@code seed}. Throws if the native side
     * reports failure — see {@link #lastError()} for why, already folded into the message.
     */
    public Handle open(String datapackPath, String dimensionId, long seed) {
        try (Arena callArena = Arena.ofConfined()) {
            MemorySegment pathSeg = callArena.allocateFrom(datapackPath);
            MemorySegment dimSeg = callArena.allocateFrom(dimensionId);
            MemorySegment result = (MemorySegment) hOpen.invokeExact(pathSeg, dimSeg, seed);
            if (result.equals(MemorySegment.NULL)) {
                throw new IllegalStateException("oxide_open failed: " + lastError());
            }
            return new Handle(result);
        } catch (RuntimeException e) {
            throw e;
        } catch (Throwable t) {
            throw new RuntimeException("oxide_open threw across the FFI boundary", t);
        }
    }

    private String lastError() {
        MemorySegment ptr = invokeAddress(hLastError, null);
        if (ptr == null || ptr.equals(MemorySegment.NULL)) {
            return "(no error message set)";
        }
        return readCString(ptr);
    }

    private static int invokeInt(MethodHandle handle, MemorySegment ptr) {
        try {
            return (int) handle.invokeExact(ptr);
        } catch (Throwable t) {
            throw new RuntimeException("native call threw across the FFI boundary", t);
        }
    }

    private static MemorySegment invokeAddress(MethodHandle handle, MemorySegment ptr) {
        try {
            return ptr == null ? (MemorySegment) handle.invokeExact() : (MemorySegment) handle.invokeExact(ptr);
        } catch (Throwable t) {
            throw new RuntimeException("native call threw across the FFI boundary", t);
        }
    }

    private static String readCString(MemorySegment ptr) {
        if (ptr.equals(MemorySegment.NULL)) {
            return "";
        }
        return ptr.reinterpret(MAX_C_STRING_LEN).getString(0);
    }

    @Override
    public void close() {
        arena.close();
    }
}
