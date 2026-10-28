//! Deep Zoom Multi-Scale Pyramid Cache with LRU Memory Management.

use std::collections::{HashMap, VecDeque};
use crate::tile_coord::TileCoordinate;

pub struct RenderedTile {
    pub coord: TileCoordinate,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct TilePyramidCache {
    tiles: HashMap<TileCoordinate, RenderedTile>,
    access_order: VecDeque<TileCoordinate>,
    tile_size: u32,
    max_cached_tiles: usize,
}

impl TilePyramidCache {
    pub fn new(tile_size: u32, max_cached_tiles: usize) -> Self {
        Self {
            tiles: HashMap::new(),
            access_order: VecDeque::new(),
            tile_size,
            max_cached_tiles: max_cached_tiles.max(16),
        }
    }

    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }

    pub fn get(&mut self, coord: &TileCoordinate) -> Option<&RenderedTile> {
        if self.tiles.contains_key(coord) {
            if let Some(pos) = self.access_order.iter().position(|c| c == coord) {
                self.access_order.remove(pos);
                self.access_order.push_back(*coord);
            }
            self.tiles.get(coord)
        } else {
            None
        }
    }

    pub fn insert(&mut self, tile: RenderedTile) {
        let coord = tile.coord;

        while self.tiles.len() >= self.max_cached_tiles && !self.access_order.is_empty() {
            if let Some(oldest) = self.access_order.pop_front() {
                self.tiles.remove(&oldest);
            }
        }

        if let Some(pos) = self.access_order.iter().position(|c| c == &coord) {
            self.access_order.remove(pos);
        }

        self.tiles.insert(coord, tile);
        self.access_order.push_back(coord);
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}
