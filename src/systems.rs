//! System functions used by the plugin for processing ldtk files.

use crate::{
    app::{
        LdtkEntityMap, LdtkIntCellMap, PhantomLdtkEntity, PhantomLdtkEntityTrait,
        PhantomLdtkIntCell, PhantomLdtkIntCellTrait,
    },
    assets::{LdtkAsset, LdtkLevel, TilesetMap},
    components::*,
    ldtk::{EntityDefinition, Level, TileInstance, TilesetDefinition, Type},
    tile_makers::*,
    utils::*,
};

use bevy::{
    prelude::*,
    render::{render_resource::TextureUsages, texture::DEFAULT_IMAGE_HANDLE},
};
use bevy_ecs_tilemap::prelude::*;
use std::collections::HashMap;

const CHUNK_SIZE: ChunkSize = ChunkSize(32, 32);

/// After external levels are loaded, this updates the corresponding [LdtkAsset]'s levels.
///
/// Note: this plugin currently doesn't support hot-reloading of external levels.
/// See <https://github.com/Trouv/bevy_ecs_ldtk/issues/1> for details.
pub fn process_external_levels(
    mut level_events: EventReader<AssetEvent<LdtkLevel>>,
    level_assets: Res<Assets<LdtkLevel>>,
    mut ldtk_assets: ResMut<Assets<LdtkAsset>>,
) {
    for event in level_events.iter() {
        // creation and deletion events should be handled by the ldtk asset events
        let mut changed_levels = Vec::<Handle<LdtkLevel>>::new();
        match event {
            AssetEvent::Created { handle } => {
                info!("External Level added!");
                changed_levels.push(handle.clone());
            }
            AssetEvent::Modified { handle } => {
                info!("External Level changed!");
                changed_levels.push(handle.clone());
            }
            _ => (),
        }

        let mut levels_to_update = Vec::new();
        for level_handle in changed_levels {
            for (ldtk_handle, ldtk_asset) in ldtk_assets.iter() {
                for (i, _) in ldtk_asset
                    .level_handles
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| **h == level_handle)
                {
                    levels_to_update.push((ldtk_handle, level_handle.clone(), i));
                }
            }
        }

        for (ldtk_handle, level_handle, level_index) in levels_to_update {
            if let Some(level) = level_assets.get(level_handle) {
                if let Some(ldtk_asset) = ldtk_assets.get_mut(ldtk_handle) {
                    if let Some(ldtk_level) = ldtk_asset.project.levels.get_mut(level_index) {
                        *ldtk_level = level.level.clone();
                    }
                }
            }
        }
    }
}

/// Detects [LdtkAsset] events and spawns levels as children of the [LdtkWorldBundle].
pub fn process_ldtk_world(
    mut commands: Commands,
    mut ldtk_events: EventReader<AssetEvent<LdtkAsset>>,
    new_ldtks: Query<&Handle<LdtkAsset>, Added<Handle<LdtkAsset>>>,
    ldtk_world_query: Query<(Entity, &Handle<LdtkAsset>, &LevelSelection)>,
    ldtk_assets: Res<Assets<LdtkAsset>>,
) {
    // This function uses code from the bevy_ecs_tilemap ldtk example
    // https://github.com/StarArawn/bevy_ecs_tilemap/blob/main/examples/ldtk/ldtk.rs
    let mut changed_ldtks = Vec::new();
    for event in ldtk_events.iter() {
        match event {
            AssetEvent::Created { handle } => {
                debug!("LDtk asset creation detected.");
                changed_ldtks.push(handle.clone());
            }
            AssetEvent::Modified { handle } => {
                debug!("LDtk asset modification detected.");
                changed_ldtks.push(handle.clone());
            }
            AssetEvent::Removed { handle } => {
                debug!("LDtk asset removal detected.");
                // if mesh was modified and removed in the same update, ignore the modification
                // events are ordered so future modification events are ok
                changed_ldtks = changed_ldtks
                    .into_iter()
                    .filter(|changed_handle| changed_handle == handle)
                    .collect();
            }
        }
    }

    for new_ldtk_handle in new_ldtks.iter() {
        changed_ldtks.push(new_ldtk_handle.clone());
    }

    for changed_ldtk in changed_ldtks {
        for (ldtk_entity, ldtk_handle, level_selection) in ldtk_world_query
            .iter()
            .filter(|(_, l, _)| **l == changed_ldtk)
        {
            commands.entity(ldtk_entity).despawn_descendants();

            if let Some(ldtk_asset) = ldtk_assets.get(ldtk_handle) {
                for (i, _) in ldtk_asset
                    .project
                    .levels
                    .iter()
                    .enumerate()
                    .filter(|(i, l)| level_selection.is_match(i, l))
                {
                    let level_entity = commands.spawn().id();
                    commands
                        .entity(level_entity)
                        .insert_bundle(LevelBundle {
                            level_handle: ldtk_asset.level_handles[i].clone(),
                            map: Map::new(i as u16, level_entity),
                            transform: Transform::default(),
                            global_transform: GlobalTransform::default(),
                        })
                        .insert(Parent(ldtk_entity));
                }
            }
        }
    }
}

/// Performs all the spawning of levels, layers, chunks, bundles, entities, tiles, etc. when an
/// LdtkLevelBundle is added.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn process_ldtk_levels(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut texture_atlases: ResMut<Assets<TextureAtlas>>,
    ldtk_assets: Res<Assets<LdtkAsset>>,
    level_assets: Res<Assets<LdtkLevel>>,
    ldtk_entity_map: NonSend<LdtkEntityMap>,
    ldtk_int_cell_map: NonSend<LdtkIntCellMap>,
    ldtk_query: Query<&Handle<LdtkAsset>>,
    mut level_query: Query<
        (Entity, &Handle<LdtkLevel>, &mut Map, &Parent),
        Added<Handle<LdtkLevel>>,
    >,
) {
    // This function uses code from the bevy_ecs_tilemap ldtk example
    // https://github.com/StarArawn/bevy_ecs_tilemap/blob/main/examples/ldtk/ldtk.rs

    for (ldtk_entity, level_handle, mut map, parent) in level_query.iter_mut() {
        if let Ok(ldtk_handle) = ldtk_query.get(parent.0) {
            if let Some(ldtk_asset) = ldtk_assets.get(ldtk_handle) {
                let tileset_definition_map: HashMap<i32, &TilesetDefinition> = ldtk_asset
                    .project
                    .defs
                    .tilesets
                    .iter()
                    .map(|t| (t.uid, t))
                    .collect();

                let entity_definition_map =
                    create_entity_definition_map(&ldtk_asset.project.defs.entities);

                if let Some(level) = level_assets.get(level_handle) {
                    spawn_level(
                        &level.level,
                        &mut commands,
                        &asset_server,
                        &mut texture_atlases,
                        &mut meshes,
                        &ldtk_entity_map,
                        &ldtk_int_cell_map,
                        &entity_definition_map,
                        &ldtk_asset.tileset_map,
                        &tileset_definition_map,
                        &mut map,
                        ldtk_entity,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_level(
    level: &Level,
    commands: &mut Commands,
    asset_server: &AssetServer,
    texture_atlases: &mut Assets<TextureAtlas>,
    meshes: &mut ResMut<Assets<Mesh>>,
    ldtk_entity_map: &LdtkEntityMap,
    ldtk_int_cell_map: &LdtkIntCellMap,
    entity_definition_map: &HashMap<i32, &EntityDefinition>,
    tileset_map: &TilesetMap,
    tileset_definition_map: &HashMap<i32, &TilesetDefinition>,
    map: &mut Map,
    ldtk_entity: Entity,
) {
    if let Some(layer_instances) = &level.layer_instances {
        let mut layer_id = 0;
        for layer_instance in layer_instances.iter().rev() {
            match layer_instance.layer_instance_type {
                Type::Entities => {
                    for entity_instance in &layer_instance.entity_instances {
                        let transform = calculate_transform_from_entity_instance(
                            entity_instance,
                            entity_definition_map,
                            level.px_hei,
                            layer_id as f32,
                        );
                        // Note: entities do not seem to be affected visually by layer offsets in
                        // the editor, so no layer offset is added to the transform here.

                        let mut entity_commands = commands.spawn();

                        let (tileset, tileset_definition) = match &entity_instance.tile {
                            Some(t) => (
                                tileset_map.get(&t.tileset_uid),
                                tileset_definition_map.get(&t.tileset_uid).copied(),
                            ),
                            None => (None, None),
                        };

                        let default_ldtk_entity: Box<dyn PhantomLdtkEntityTrait> =
                            Box::new(PhantomLdtkEntity::<EntityInstanceBundle>::new());

                        ldtk_map_get_or_default(
                            layer_instance.identifier.clone(),
                            entity_instance.identifier.clone(),
                            &default_ldtk_entity,
                            ldtk_entity_map,
                        )
                        .evaluate(
                            &mut entity_commands,
                            entity_instance,
                            layer_instance,
                            tileset,
                            tileset_definition,
                            asset_server,
                            texture_atlases,
                        );

                        entity_commands
                            .insert(transform)
                            .insert(GlobalTransform::default())
                            .insert(Parent(ldtk_entity));
                    }
                }
                _ => {
                    // The remaining layers have a lot of shared code.
                    // This is because:
                    // 1. There is virtually no difference between AutoTile and Tile layers
                    // 2. IntGrid layers can sometimes have AutoTile functionality

                    let map_size = MapSize(
                        (layer_instance.c_wid as f32 / CHUNK_SIZE.0 as f32).ceil() as u32,
                        (layer_instance.c_hei as f32 / CHUNK_SIZE.1 as f32).ceil() as u32,
                    );

                    let tileset_definition = layer_instance
                        .tileset_def_uid
                        .map(|u| tileset_definition_map.get(&u).unwrap());

                    let tile_size = match tileset_definition {
                        Some(tileset_definition) => TileSize(
                            tileset_definition.tile_grid_size as f32,
                            tileset_definition.tile_grid_size as f32,
                        ),
                        None => TileSize(
                            layer_instance.grid_size as f32,
                            layer_instance.grid_size as f32,
                        ),
                    };

                    let texture_size = match tileset_definition {
                        Some(tileset_definition) => TextureSize(
                            tileset_definition.px_wid as f32,
                            tileset_definition.px_hei as f32,
                        ),
                        None => TextureSize(0., 0.),
                    };

                    let mut settings =
                        LayerSettings::new(map_size, CHUNK_SIZE, tile_size, texture_size);

                    if let Some(tileset_definition) = tileset_definition {
                        settings.grid_size = Vec2::splat(layer_instance.grid_size as f32);
                        settings.tile_spacing = Vec2::splat(tileset_definition.spacing as f32);
                    }

                    // The change to the settings.grid_size above is supposed to help handle cases
                    // where the tileset's tile size and the layer's tile size are different.
                    // However, changing the grid_size doesn't have any affect with the current
                    // bevy_ecs_tilemap, so the workaround is to scale up the entire layer.
                    let layer_scale = (settings.grid_size
                        / Vec2::new(settings.tile_size.0 as f32, settings.tile_size.1 as f32))
                    .extend(1.);

                    let image_handle = match tileset_definition {
                        Some(tileset_definition) => {
                            tileset_map.get(&tileset_definition.uid).unwrap().clone()
                        }
                        None => DEFAULT_IMAGE_HANDLE.typed(),
                    };

                    let mut grid_tiles = layer_instance.grid_tiles.clone();
                    grid_tiles.extend(layer_instance.auto_layer_tiles.clone());

                    for (i, grid_tiles) in layer_grid_tiles(grid_tiles).into_iter().enumerate() {
                        let layer_entity = if layer_instance.layer_instance_type == Type::IntGrid {
                            // The current spawning of IntGrid layers doesn't allow using
                            // LayerBuilder::new_batch().
                            // So, the actual LayerBuilder usage diverges greatly here

                            let (mut layer_builder, layer_entity) = LayerBuilder::<TileBundle>::new(
                                commands,
                                settings,
                                map.id,
                                layer_id as u16,
                            );

                            match tileset_definition {
                                Some(_) => {
                                    let tile_maker = tile_pos_to_tile_maker(
                                        layer_instance.c_hei,
                                        layer_instance.grid_size,
                                        grid_tiles,
                                    );

                                    set_all_tiles_with_func(
                                        &mut layer_builder,
                                        tile_pos_to_tile_bundle_maker(tile_maker),
                                    );
                                }
                                None => {
                                    set_all_tiles_with_func(
                                        &mut layer_builder,
                                        tile_pos_to_tile_bundle_if_int_grid_nonzero_maker(
                                            tile_pos_to_invisible_tile,
                                            &layer_instance.int_grid_csv,
                                            layer_instance.c_wid,
                                            layer_instance.c_hei,
                                        ),
                                    );
                                }
                            }

                            if i == 0 {
                                for (i, value) in layer_instance
                                    .int_grid_csv
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, v)| **v != 0)
                                {
                                    let tile_pos = int_grid_index_to_tile_pos(
                                i,
                                layer_instance.c_wid as u32,
                                layer_instance.c_hei as u32,
                            ).expect("int_grid_csv indices should be within the bounds of 0..(layer_widthd * layer_height)");

                                    let tile_entity =
                                        layer_builder.get_tile_entity(commands, tile_pos).unwrap();

                                    let mut translation = tile_pos_to_translation_centered(
                                        tile_pos,
                                        IVec2::splat(layer_instance.grid_size),
                                    )
                                    .extend(layer_id as f32);

                                    translation /= layer_scale;

                                    let mut entity_commands = commands.entity(tile_entity);

                                    let default_ldtk_int_cell: Box<dyn PhantomLdtkIntCellTrait> =
                                        Box::new(PhantomLdtkIntCell::<IntGridCellBundle>::new());

                                    ldtk_map_get_or_default(
                                        layer_instance.identifier.clone(),
                                        *value,
                                        &default_ldtk_int_cell,
                                        ldtk_int_cell_map,
                                    )
                                    .evaluate(
                                        &mut entity_commands,
                                        IntGridCell { value: *value },
                                        layer_instance,
                                    );

                                    entity_commands
                                        .insert(Transform::from_translation(translation))
                                        .insert(GlobalTransform::default())
                                        .insert(Parent(layer_entity));
                                }
                            }

                            let layer_bundle =
                                layer_builder.build(commands, meshes, image_handle.clone());

                            commands.entity(layer_entity).insert_bundle(layer_bundle);

                            layer_entity
                        } else {
                            let tile_maker = tile_pos_to_tile_maker(
                                layer_instance.c_hei,
                                layer_instance.grid_size,
                                grid_tiles,
                            );

                            LayerBuilder::<TileBundle>::new_batch(
                                commands,
                                settings,
                                meshes,
                                image_handle.clone(),
                                map.id,
                                layer_id as u16,
                                tile_pos_to_tile_bundle_maker(tile_maker),
                            )
                        };

                        let layer_offset = Vec3::new(
                            layer_instance.px_total_offset_x as f32,
                            -layer_instance.px_total_offset_y as f32,
                            0.,
                        );

                        commands.entity(layer_entity).insert(
                            Transform::from_translation(layer_offset).with_scale(layer_scale),
                        );

                        map.add_layer(commands, layer_id as u16, layer_entity);
                        layer_id += 1;
                    }
                }
            }
        }
    }
}

fn layer_grid_tiles(grid_tiles: Vec<TileInstance>) -> Vec<Vec<TileInstance>> {
    let mut layer = Vec::new();
    let mut overflow = Vec::new();
    for tile in grid_tiles {
        if layer.iter().any(|t: &TileInstance| t.px == tile.px) {
            overflow.push(tile);
        } else {
            layer.push(tile);
        }
    }

    let mut layered_grid_tiles = vec![layer];
    if !overflow.is_empty() {
        layered_grid_tiles.extend(layer_grid_tiles(overflow));
    }

    layered_grid_tiles
}

pub fn set_ldtk_texture_filters_to_nearest(
    mut texture_events: EventReader<AssetEvent<Image>>,
    mut textures: ResMut<Assets<Image>>,
    ldtk_assets: Res<Assets<LdtkAsset>>,
) {
    // Based on
    // https://github.com/StarArawn/bevy_ecs_tilemap/blob/main/examples/helpers/texture.rs,
    // except it only applies to the ldtk tilesets.
    for event in texture_events.iter() {
        if let AssetEvent::Created { handle } = event {
            let mut set_texture_filters_to_nearest = false;

            for (_, ldtk_asset) in ldtk_assets.iter() {
                if ldtk_asset.tileset_map.iter().any(|(_, v)| v == handle) {
                    set_texture_filters_to_nearest = true;
                    break;
                }
            }

            if set_texture_filters_to_nearest {
                if let Some(mut texture) = textures.get_mut(handle) {
                    texture.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC
                        | TextureUsages::COPY_DST;
                }
            }
        }
    }
}
