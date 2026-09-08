package dev.oxide.plugin.pacside;

import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.plugin.java.JavaPlugin;
import org.bukkit.util.Vector;

import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Predictive Trajectory Chunk Prefetcher for Elytra and High-Speed Player Travel.
 *
 * <p>Tracks player movement vectors (dx, dz) and predictively pre-loads the forward
 * chunk cone into memory before the client arrives, eliminating Elytra loading lag.
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
        if (last != null && (now - last) < 500) {
            // Rate limit prefetch evaluations to at most twice per second per player
            return;
        }
        lastPrefetch.put(uuid, now);

        // Normalize direction vector
        double speed = Math.sqrt(speedSq);
        double dirX = dx / speed;
        double dirZ = dz / speed;

        World world = player.getWorld();
        int playerChunkX = to.getBlockX() >> 4;
        int playerChunkZ = to.getBlockZ() >> 4;

        // Prefetch distance: 3 to 6 chunks ahead based on speed
        int lookahead = player.isGliding() ? 7 : (player.isSprinting() ? 4 : 3);

        for (int dist = 2; dist <= lookahead; dist++) {
            int targetX = playerChunkX + (int) Math.round(dirX * dist);
            int targetZ = playerChunkZ + (int) Math.round(dirZ * dist);

            // Pre-load on Folia region scheduler
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
