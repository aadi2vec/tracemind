import sqlite3
import json
import logging
from typing import Dict, List, Tuple, Optional

logger = logging.getLogger(__name__)

class SQLiteClusterPersistence:
    """SQLite backend for ClusterStore to persist embeddings and clusters."""
    
    def __init__(self, db_path: str = "cluster_store.db"):
        self.db_path = db_path
        self._init_db()

    def _init_db(self):
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            # Entities table
            cursor.execute("""
                CREATE TABLE IF NOT EXISTS entities (
                    entity_id TEXT PRIMARY KEY,
                    embedding BLOB,
                    cluster_id INTEGER DEFAULT -1
                )
            """)
            conn.commit()

    def save_entity(self, entity_id: str, embedding: List[float], cluster_id: int = -1):
        """Save or update an entity and its cluster assignment."""
        embedding_json = json.dumps(embedding)
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute("""
                INSERT INTO entities (entity_id, embedding, cluster_id)
                VALUES (?, ?, ?)
                ON CONFLICT(entity_id) DO UPDATE SET
                    embedding = excluded.embedding,
                    cluster_id = excluded.cluster_id
            """, (entity_id, embedding_json, cluster_id))
            conn.commit()

    def save_clusters(self, entity_cluster_map: Dict[str, int]):
        """Batch update cluster IDs for multiple entities."""
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            batch = [(cid, eid) for eid, cid in entity_cluster_map.items()]
            cursor.executemany("""
                UPDATE entities SET cluster_id = ? WHERE entity_id = ?
            """, batch)
            conn.commit()

    def load_all(self) -> Tuple[List[str], List[List[float]], Dict[str, int]]:
        """Load all entities, embeddings, and cluster assignments."""
        entity_ids = []
        embeddings = []
        cluster_map = {}
        
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute("SELECT entity_id, embedding, cluster_id FROM entities")
            for row in cursor.fetchall():
                eid, emb_json, cid = row
                entity_ids.append(eid)
                embeddings.append(json.loads(emb_json))
                cluster_map[eid] = cid
                
        return entity_ids, embeddings, cluster_map
