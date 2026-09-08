package dev.oxide.plugin.pacside;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.plugin.java.JavaPlugin;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Predictive Trajectory Chunk Prefetcher & Radius Loader.
 *
 * <p>Supports both real-time dynamic Elytra trajectory prefetching and on-demand
 * radius prefetching (/oxide pacside fetch radius <blocks>).
 */
public final class PacsidePrefetcher implements Listener {

    private final JavaPlugin plugin;
    private final Map<UUID, Long> lastPrefetch = new ConcurrentHashMap<>();
    private final AtomicLong prefetchedChunks = new AtomicLong();
    private boolean enabled = true;

    public PacsidePrefetcher(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    public boolean isEnabled() {
        return enabled;
    }

    public void setEnabled(boolean enabled) {
        this.enabled = enabled;
    }

    public long getPrefetchedCount() {
        return prefetchedChunks.get();
    }

    /**
     * Asynchronously prefetches and warms up all chunks in a radius around the player.
     *
     * @param player The player requesting the prefetch.
     * @param radiusValue Radius in blocks (if > 64) or in chunks (if <= 64).
     */
    public void prefetchRadius(Player player, int radiusValue) {
        int chunkRadius = radiusValue > 64 ? (radiusValue / 16) : radiusValue;
        chunkRadius = Math.max(1, Math.min(chunkRadius, 128)); // Bound between 1 and 128 chunks radius (up to 2048 blocks)

        Location loc = player.getLocation();
        World world = loc.getWorld();
        int centerChunkX = loc.getBlockX() >> 4;
        int centerChunkZ = loc.getBlockZ() >> 4;

        int totalChunks = (2 * chunkRadius + 1) * (2 * chunkRadius + 1);
        player.sendMessage(Component.text("[Pacside] Prefetching " + totalChunks + " chunks (radius "
                + (chunkRadius * 16) + " blocks) around you...", NamedTextColor.GOLD));

        long startTime = System.currentTimeMillis();
        AtomicInteger loadedCount = new AtomicInteger();
        List<CompletableFuture<?>> futures = new ArrayList<>(totalChunks);

        for (int dz = -chunkRadius; dz <= chunkRadius; dz++) {
            for (int dx = -chunkRadius; dx <= chunkRadius; dx++) {
                int cx = centerChunkX + dx;
                int cz = centerChunkZ + dz;

                CompletableFuture<?> f = world.getChunkAtAsync(cx, cz, false).thenAccept(chunk -> {
                    if (chunk != null) {
                        loadedCount.incrementAndGet();
                        prefetchedChunks.incrementAndGet();
                    }
                });
                futures.add(f);
            }
        }

        CompletableFuture.allOf(futures.toArray(new CompletableFuture[0])).thenRun(() -> {
            long duration = System.currentTimeMillis() - startTime;
            player.sendMessage(Component.text("[Pacside] Successfully pre-warmed " + loadedCount.get()
                    + " chunks in " + duration + " ms (" + String.format("%.1f", (loadedCount.get() * 1000.0 / Math.max(1, duration)))
                    + " CPS)!", NamedTextColor.GREEN));
        });
    }

    @EventHandler(priority = EventPriority.MONITOR, ignoreCancelled = true)
    public void onPlayerMove(PlayerMoveEvent event) {
        if (!enabled) return;

        Location from = event.getFrom();
        Location to = event.getTo();
        if (to == null) return;

        double dx = to.getX() - from.getX();
        double dz = to.getZ() - from.getZ();
        double speedSq = dx * dx + dz * dz;

        // Trigger prefetching only if player is moving with meaningful velocity (> 0.25 blocks/tick = > 5 m/s)
        if (speedSq < 0.0625) return;

        Player player = event.getPlayer();
        UUID uuid = player.getUniqueId();
        long now = System.currentTimeMillis();

        Long last = lastPrefetch.get(uuid);
        if (last != null && (now - last) < 400) {
            // Rate limit prefetch evaluations to at most 2.5 times per second per player
            return;
        }
        lastPrefetch.put(uuid, now);

        double speed = Math.sqrt(speedSq);
        double dirX = dx / speed;
        double dirZ = dz / speed;

        World world = player.getWorld();
        int playerChunkX = to.getBlockX() >> 4;
        int playerChunkZ = to.getBlockZ() >> 4;

        int lookahead = player.isGliding() ? 8 : (player.isSprinting() ? 5 : 3);

        for (int dist = 2; dist <= lookahead; dist++) {
            int targetX = playerChunkX + (int) Math.round(dirX * dist);
            int targetZ = playerChunkZ + (int) Math.round(dirZ * dist);

            world.getChunkAtAsync(targetX, targetZ, false).thenAccept(chunk -> {
                if (chunk != null) {
                    prefetchedChunks.incrementAndGet();
                }
            });

            // Preload 1 block wide lateral cone
            int sideX = playerChunkX + (int) Math.round((dirX * dist) - (dirZ * 1.0));
            int sideZ = playerChunkZ + (int) Math.round((dirZ * dist) + (dirX * 1.0));
            world.getChunkAtAsync(sideX, sideZ, false).thenAccept(chunk -> {
                if (chunk != null) {
                    prefetchedChunks.incrementAndGet();
                }
            });
        }
    }
}
