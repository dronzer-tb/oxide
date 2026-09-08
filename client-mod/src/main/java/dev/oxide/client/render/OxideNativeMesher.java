package dev.oxide.client.render;

import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;

/**
 * Foreign Function & Memory downcalls to Rust `oxide-render` library.
 */
public final class OxideNativeMesher {

    private static MethodHandle meshSectionHandle;
    private static MethodHandle freeMeshHandle;
    private static MethodHandle cullSectionsHandle;
    private static boolean available = false;

    public static void init() throws NoSuchMethodException, IllegalAccessException {
        Linker linker = Linker.nativeLinker();
        SymbolLookup lookup = SymbolLookup.loaderLookup();

        MemorySegment meshSectionSym = lookup.find("oxide_render_mesh_section")
                .orElseThrow(() -> new UnsatisfiedLinkError("oxide_render_mesh_section not found"));
        MemorySegment freeMeshSym = lookup.find("oxide_render_free_mesh")
                .orElseThrow(() -> new UnsatisfiedLinkError("oxide_render_free_mesh not found"));
        MemorySegment cullSym = lookup.find("oxide_render_cull_sections")
                .orElseThrow(() -> new UnsatisfiedLinkError("oxide_render_cull_sections not found"));

        FunctionDescriptor meshDesc = FunctionDescriptor.of(
                ValueLayout.ADDRESS,          // return RawMeshHandle*
                ValueLayout.ADDRESS,          // blocks_ptr (const u16*)
                ValueLayout.ADDRESS,          // out_vertices_ptr (const PackedVertex**)
                ValueLayout.ADDRESS,          // out_vertex_count (size_t*)
                ValueLayout.ADDRESS,          // out_indices_ptr (const u32**)
                ValueLayout.ADDRESS           // out_index_count (size_t*)
        );

        FunctionDescriptor freeDesc = FunctionDescriptor.ofVoid(
                ValueLayout.ADDRESS           // handle (RawMeshHandle*)
        );

        FunctionDescriptor cullDesc = FunctionDescriptor.of(
                ValueLayout.JAVA_LONG,        // return visible_count (size_t)
                ValueLayout.ADDRESS,          // frustum_ptr (const Frustum*)
                ValueLayout.ADDRESS,          // aabbs_ptr (const SectionAABB*)
                ValueLayout.JAVA_LONG,        // count (size_t)
                ValueLayout.ADDRESS           // visible_out (u8*)
        );

        meshSectionHandle = linker.downcallHandle(meshSectionSym, meshDesc);
        freeMeshHandle = linker.downcallHandle(freeMeshSym, freeDesc);
        cullSectionsHandle = linker.downcallHandle(cullSym, cullDesc);

        available = true;
    }

    public static boolean isAvailable() {
        return available;
    }

    public record NativeMeshResult(MemorySegment handle, MemorySegment vertices, long vertexCount, MemorySegment indices, long indexCount) implements AutoCloseable {
        @Override
        public void close() {
            if (handle != null && !handle.equals(MemorySegment.NULL)) {
                try {
                    freeMeshHandle.invokeExact(handle);
                } catch (Throwable ignored) {}
            }
        }
    }

    /**
     * Meshes a 4096-block section using SIMD Greedy Meshing in Rust.
     */
    public static NativeMeshResult meshSection(short[] blocks) {
        if (!available || blocks == null || blocks.length != 4096) {
            return null;
        }

        try (Arena arena = Arena.ofConfined()) {
            MemorySegment blocksSeg = arena.allocate(ValueLayout.JAVA_SHORT, 4096);
            MemorySegment.copy(blocks, 0, blocksSeg, ValueLayout.JAVA_SHORT, 0, 4096);

            MemorySegment outVerticesPtr = arena.allocate(ValueLayout.ADDRESS);
            MemorySegment outVertexCount = arena.allocate(ValueLayout.JAVA_LONG);
            MemorySegment outIndicesPtr = arena.allocate(ValueLayout.ADDRESS);
            MemorySegment outIndexCount = arena.allocate(ValueLayout.JAVA_LONG);

            MemorySegment handle = (MemorySegment) meshSectionHandle.invokeExact(
                    blocksSeg,
                    outVerticesPtr,
                    outVertexCount,
                    outIndicesPtr,
                    outIndexCount
            );

            if (handle.equals(MemorySegment.NULL)) {
                return null;
            }

            MemorySegment vertPtr = outVerticesPtr.get(ValueLayout.ADDRESS, 0);
            long vertCount = outVertexCount.get(ValueLayout.JAVA_LONG, 0);
            MemorySegment idxPtr = outIndicesPtr.get(ValueLayout.ADDRESS, 0);
            long idxCount = outIndexCount.get(ValueLayout.JAVA_LONG, 0);

            return new NativeMeshResult(handle, vertPtr, vertCount, idxPtr, idxCount);
        } catch (Throwable t) {
            return null;
        }
    }
}
