package dev.oxide.plugin.flight;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.TextDecoration;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.event.block.Action;
import org.bukkit.event.entity.EntityToggleGlideEvent;
import org.bukkit.event.entity.PlayerDeathEvent;
import org.bukkit.event.player.PlayerInteractEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.player.PlayerRespawnEvent;
import org.bukkit.inventory.EquipmentSlot;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.PlayerInventory;
import org.bukkit.inventory.meta.FireworkMeta;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.plugin.java.JavaPlugin;

/**
 * Automatically equips joining players with an Unbreakable Elytra
 * and provides infinite auto-replenishing fireworks for high-speed flight testing.
 */
public final class ElytraFlightListener implements Listener {

    private final JavaPlugin plugin;

    public ElytraFlightListener(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    private static ItemStack createOxideElytra() {
        ItemStack elytra = new ItemStack(Material.ELYTRA);
        ItemMeta meta = elytra.getItemMeta();
        if (meta != null) {
            meta.displayName(Component.text("Oxide Flight Elytra", NamedTextColor.AQUA, TextDecoration.BOLD));
            meta.setUnbreakable(true);
            meta.addEnchant(Enchantment.UNBREAKING, 3, true);
            meta.addEnchant(Enchantment.MENDING, 1, true);
            elytra.setItemMeta(meta);
        }
        return elytra;
    }

    private static ItemStack createOxideRockets(int amount) {
        ItemStack rockets = new ItemStack(Material.FIREWORK_ROCKET, amount);
        FireworkMeta meta = (FireworkMeta) rockets.getItemMeta();
        if (meta != null) {
            meta.setPower(3);
            meta.displayName(Component.text("Oxide Infinite Rocket", NamedTextColor.GOLD, TextDecoration.BOLD));
            rockets.setItemMeta(meta);
        }
        return rockets;
    }

    private void ensureFlightKit(Player player) {
        PlayerInventory inv = player.getInventory();

        // 1. Ensure Elytra is equipped or in inventory
        ItemStack chest = inv.getChestplate();
        if (chest == null || chest.getType() != Material.ELYTRA) {
            if (chest != null && chest.getType() != Material.AIR) {
                inv.addItem(chest); // Stash old chestplate
            }
            inv.setChestplate(createOxideElytra());
        }

        // 2. Ensure rockets exist in inventory
        int rocketCount = 0;
        for (ItemStack item : inv.getContents()) {
            if (item != null && item.getType() == Material.FIREWORK_ROCKET) {
                rocketCount += item.getAmount();
            }
        }

        if (rocketCount < 16) {
            inv.addItem(createOxideRockets(64));
        }
    }

    @EventHandler(priority = EventPriority.HIGH)
    public void onPlayerJoin(PlayerJoinEvent event) {
        Player player = event.getPlayer();
        ensureFlightKit(player);
        player.sendMessage(Component.text("⚡ [Oxide] Flight kit equipped: Unbreakable Elytra & Infinite Rockets ready!", NamedTextColor.AQUA));
    }

    @EventHandler(priority = EventPriority.HIGH)
    public void onPlayerRespawn(PlayerRespawnEvent event) {
        Player player = event.getPlayer();
        plugin.getServer().getRegionScheduler().runDelayed(plugin, player.getLocation(), task -> {
            if (player.isOnline()) {
                ensureFlightKit(player);
            }
        }, 1L);
    }

    @EventHandler(priority = EventPriority.MONITOR)
    public void onToggleGlide(EntityToggleGlideEvent event) {
        if (event.getEntity() instanceof Player player && event.isGliding()) {
            ensureFlightKit(player);
        }
    }

    @EventHandler(priority = EventPriority.HIGHEST)
    public void onPlayerInteract(PlayerInteractEvent event) {
        Player player = event.getPlayer();
        Action action = event.getAction();

        if (action == Action.RIGHT_CLICK_AIR || action == Action.RIGHT_CLICK_BLOCK) {
            ItemStack item = event.getItem();
            if (item != null && item.getType() == Material.FIREWORK_ROCKET) {
                // Check remaining rockets
                int totalRockets = 0;
                PlayerInventory inv = player.getInventory();
                for (ItemStack stack : inv.getContents()) {
                    if (stack != null && stack.getType() == Material.FIREWORK_ROCKET) {
                        totalRockets += stack.getAmount();
                    }
                }

                // If running low, auto-replenish to full 64 stack immediately
                if (totalRockets <= 4) {
                    inv.addItem(createOxideRockets(64));
                }
            }
        }
    }
}
