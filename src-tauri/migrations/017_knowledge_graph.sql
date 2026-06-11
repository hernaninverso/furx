-- F3.4 — Knowledge Graph: entities and relations between memories.
-- Enables cross-source semantic connections between memories from different CLIs.

CREATE TABLE IF NOT EXISTS memory_entities (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  entity_type TEXT NOT NULL DEFAULT 'term',   -- term | url | person | project | technology
  metadata TEXT DEFAULT '{}',                   -- JSON blob for extra properties
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_memory_entities_name ON memory_entities(name);
CREATE INDEX IF NOT EXISTS idx_memory_entities_type ON memory_entities(entity_type);

CREATE TABLE IF NOT EXISTS memory_relations (
  id TEXT PRIMARY KEY NOT NULL,
  source_entity_id TEXT NOT NULL,
  target_entity_id TEXT NOT NULL,
  relation_type TEXT NOT NULL DEFAULT 'related', -- related | similar | mentions | appears_with
  weight REAL NOT NULL DEFAULT 1.0,
  source TEXT DEFAULT 'auto',                     -- auto | manual | ump
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  FOREIGN KEY (source_entity_id) REFERENCES memory_entities(id),
  FOREIGN KEY (target_entity_id) REFERENCES memory_entities(id)
);

CREATE INDEX IF NOT EXISTS idx_memory_relations_source ON memory_relations(source_entity_id);
CREATE INDEX IF NOT EXISTS idx_memory_relations_target ON memory_relations(target_entity_id);
CREATE INDEX IF NOT EXISTS idx_memory_relations_type ON memory_relations(relation_type);
CREATE INDEX IF NOT EXISTS idx_memory_relations_weight ON memory_relations(weight DESC);
