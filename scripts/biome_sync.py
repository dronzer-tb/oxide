#!/usr/bin/env python3
"""
Season 5 Biome Sync Generator for Geyser & Bedrock
Extracts custom biome colors (foliage, grass, water, fog, sky) from all installed
server datapacks (Terralith, Incendium, etc.) and generates a Bedrock Client Resource
Pack for Geyser so Bedrock and Java colors are 100% synchronized.
"""

import os
import sys
import zipfile
import json
import uuid
import argparse

def to_hex(color_val):
    if color_val is None:
        return None
    if isinstance(color_val, str):
        if color_val.startswith("#"):
            return color_val.lower()
        try:
            val = int(color_val)
            return f"#{val:06x}".lower()
        except Exception:
            return color_val.lower()
    if isinstance(color_val, (int, float)):
        return f"#{int(color_val):06x}".lower()
    return None

def sync_biomes(datapacks_dir, geyser_packs_dir, output_pack_name="Season5_BiomeSync.mcpack"):
    os.makedirs(geyser_packs_dir, exist_ok=True)
    all_biomes = {}

    if not os.path.exists(datapacks_dir):
        print(f"Error: datapacks directory not found at {datapacks_dir}")
        return False

    for fname in sorted(os.listdir(datapacks_dir)):
        fpath = os.path.join(datapacks_dir, fname)
        if fname.endswith(".zip"):
            try:
                with zipfile.ZipFile(fpath, "r") as z:
                    for item in z.namelist():
                        if "worldgen/biome/" in item and "tags/" not in item and item.endswith(".json"):
                            parts = item.split("/")
                            if len(parts) >= 4 and parts[0] == "data" and parts[2] == "worldgen" and parts[3] == "biome":
                                ns = parts[1]
                                bname = parts[4].replace(".json", "")
                                full_id = f"{ns}:{bname}"
                                try:
                                    data = json.loads(z.read(item).decode())
                                    all_biomes[full_id] = {
                                        "source": fname,
                                        "temperature": data.get("temperature", 0.5),
                                        "downfall": data.get("downfall", 0.5),
                                        "effects": data.get("effects", {})
                                    }
                                except Exception:
                                    pass
            except Exception as e:
                print(f"Warning: could not read {fname}: {e}")
        elif os.path.isdir(fpath):
            data_dir = os.path.join(fpath, "data")
            if os.path.exists(data_dir):
                for ns in os.listdir(data_dir):
                    biome_dir = os.path.join(data_dir, ns, "worldgen", "biome")
                    if os.path.exists(biome_dir):
                        for bfile in os.listdir(biome_dir):
                            if bfile.endswith(".json"):
                                bname = bfile.replace(".json", "")
                                full_id = f"{ns}:{bname}"
                                try:
                                    with open(os.path.join(biome_dir, bfile), "r") as bf:
                                        data = json.load(bf)
                                        all_biomes[full_id] = {
                                            "source": fname,
                                            "temperature": data.get("temperature", 0.5),
                                            "downfall": data.get("downfall", 0.5),
                                            "effects": data.get("effects", {})
                                        }
                                except Exception:
                                    pass

    print(f"[BiomeSync] Scanned {len(all_biomes)} biomes across datapacks.")

    biomes_client = {}
    for biome_id, info in all_biomes.items():
        eff = info["effects"]
        client_def = {}
        
        water = to_hex(eff.get("water_color"))
        water_fog = to_hex(eff.get("water_fog_color"))
        fog = to_hex(eff.get("fog_color"))
        foliage = to_hex(eff.get("foliage_color"))
        grass = to_hex(eff.get("grass_color"))
        sky = to_hex(eff.get("sky_color"))
        
        if water:
            client_def["water_surface_color"] = water
            client_def["water_surface_transparency"] = 0.55
        if water_fog:
            client_def["water_fog_color"] = water_fog
        if fog:
            client_def["fog_color"] = fog
        if foliage:
            client_def["foliage_color"] = foliage
        if grass:
            client_def["grass_color"] = grass
        if sky:
            client_def["sky_color"] = sky
            
        if client_def:
            biomes_client[biome_id] = client_def

    manifest = {
        "format_version": 2,
        "header": {
            "name": "Season 5 Biome Sync (Terralith & Datapacks)",
            "description": f"Synchronizes {len(biomes_client)} custom datapack biome colors (foliage, grass, water, fog) to Bedrock via Geyser.",
            "uuid": str(uuid.uuid5(uuid.NAMESPACE_DNS, "biome.sync.season5.header")),
            "version": [1, 0, 0],
            "min_engine_version": [1, 20, 0]
        },
        "modules": [
            {
                "type": "resources",
                "uuid": str(uuid.uuid5(uuid.NAMESPACE_DNS, "biome.sync.season5.module")),
                "version": [1, 0, 0]
            }
        ]
    }

    client_biomes_payload = {
        "biomes_client": {
            "biomes": biomes_client
        }
    }

    mcpack_path = os.path.join(geyser_packs_dir, output_pack_name)
    with zipfile.ZipFile(mcpack_path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("manifest.json", json.dumps(manifest, indent=2))
        z.writestr("biomes_client.json", json.dumps(client_biomes_payload, indent=2))

    print(f"[BiomeSync] Successfully generated Bedrock resource pack: {mcpack_path}")
    print(f"[BiomeSync] Pack contains {len(biomes_client)} synchronized biomes.")
    return True

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Biome Sync generator for Geyser & Bedrock")
    parser.add_argument("--datapacks", default="/var/lib/elytra/volumes/0fce73e7-e8d0-4f7a-9625-2a55bf585cfe/world/datapacks", help="Path to server datapacks folder")
    parser.add_argument("--geyser-packs", default="/var/lib/elytra/volumes/fcd036ec-3e0b-46b7-9099-1a9ea8736f30/packs", help="Path to Geyser packs folder")
    args = parser.parse_args()

    sync_biomes(args.datapacks, args.geyser_packs)
