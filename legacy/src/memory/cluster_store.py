"""
Soft Semantic Clustering Layer (Phase B)
=========================================
Maintains an incremental HDBSCAN cluster index over entity embeddings.
Augments graph + vector retrieval with cluster-neighbourhood expansion
so the Recall Engine can surface related entities even without explicit
graph edges.
"""
import logging
import threading
from typing import Dict, List, Optional, Tuple
import numpy as np
from .cluster_persistence import SQLiteClusterPersistence

logger = logging.getLogger(__name__)

# Optional heavy deps — graceful degradation if not installed
try:
    import faiss  # type: ignore
    FAISS_OK = True
except ImportError:
    FAISS_OK = False
    logger.warning("faiss-cpu not installed; ClusterStore will use brute-force cosine.")

try:
    from sklearn.cluster import HDBSCAN  # type: ignore
    HDBSCAN_OK = True
except ImportError:
    HDBSCAN_OK = False
    logger.warning("scikit-learn HDBSCAN not available; clustering disabled.")


class ClusterStore:
    """
    Entity embedding → soft cluster index.

    Usage::
        cs = ClusterStore(db_path="my_clusters.db")
        cs.add_entity("TSLA", [0.1, 0.2, ...])
        cs.fit()
        neighbours = cs.expand_query(query_vec, top_k=10)
    """

    def __init__(self, embedding_dim: Optional[int] = None, min_cluster_size: int = 3, db_path: str = "cluster_store.db"):
        self.embedding_dim = embedding_dim
        self.min_cluster_size = min_cluster_size
        self.persistence = SQLiteClusterPersistence(db_path)

        # Load existing data from SQLite
        self._entity_ids, self._embeddings, self._entity_cluster = self.persistence.load_all()
        
        # Infer dimension if not provided but we have data
        if self._embeddings and self.embedding_dim is None:
            self.embedding_dim = len(self._embeddings[0])

        # Rebuild _clusters lookup
        self._clusters: Dict[int, List[str]] = {}
        for eid, cid in self._entity_cluster.items():
            if cid not in self._clusters:
                self._clusters[cid] = []
            self._clusters[cid].append(eid)

        self._lock = threading.Lock()
        self._fitted = len(self._entity_cluster) > 0

    # ── Ingestion ─────────────────────────────────────────────────────────

    def add_entity(self, entity_id: str, embedding: List[float]) -> None:
        """Register an entity embedding. Call `fit()` periodically to rebuild clusters."""
        with self._lock:
            if self.embedding_dim is None:
                self.embedding_dim = len(embedding)
            
            if entity_id in self._entity_ids:
                # Update existing
                idx = self._entity_ids.index(entity_id)
                self._embeddings[idx] = embedding
            else:
                self._entity_ids.append(entity_id)
                self._embeddings.append(embedding)
            
            # Persist immediately
            cid = self._entity_cluster.get(entity_id, -1)
            self.persistence.save_entity(entity_id, embedding, cid)
            self._fitted = False

    def fit(self) -> int:
        """
        Rebuild HDBSCAN clusters over all registered embeddings.
        Returns the number of clusters found (excluding noise).
        """
        with self._lock:
            if len(self._embeddings) < self.min_cluster_size:
                logger.debug("Not enough entities to cluster (%d)", len(self._embeddings))
                return 0

            X = np.array(self._embeddings, dtype=np.float32)
            # Normalise for cosine similarity
            norms = np.linalg.norm(X, axis=1, keepdims=True) + 1e-8
            X = X / norms

            if HDBSCAN_OK:
                hdb = HDBSCAN(
                    min_cluster_size=self.min_cluster_size,
                    metric="euclidean",
                    store_centers="centroid",
                )
                labels = hdb.fit_predict(X)
            else:
                # Fallback: assign everything to cluster 0
                labels = np.zeros(len(X), dtype=int)

            # Rebuild lookup tables
            self._clusters = {}
            self._entity_cluster = {}
            for idx, label in enumerate(labels):
                eid = self._entity_ids[idx]
                self._entity_cluster[eid] = int(label)
                if int(label) not in self._clusters:
                    self._clusters[int(label)] = []
                self._clusters[int(label)].append(eid)

            # Persist cluster assignments
            self.persistence.save_clusters(self._entity_cluster)

            self._fitted = True
            n_clusters = len([k for k in self._clusters if k != -1])
            logger.info("ClusterStore: fit complete — %d clusters over %d entities", n_clusters, len(self._entity_ids))
            return n_clusters

    # ── Retrieval ─────────────────────────────────────────────────────────

    def expand_query(
        self,
        query_embedding: List[float],
        top_k: int = 15,
    ) -> List[str]:
        """
        Find the cluster(s) nearest to the query and return their member entity IDs.
        """
        with self._lock:
            if not self._fitted or not self._embeddings:
                return []

            q = np.array(query_embedding, dtype=np.float32)
            q = q / (np.linalg.norm(q) + 1e-8)

            X = np.array(self._embeddings, dtype=np.float32)
            norms = np.linalg.norm(X, axis=1, keepdims=True) + 1e-8
            X = X / norms

            # Cosine similarity via dot product
            sims = X @ q
            top_indices = np.argsort(sims)[::-1][:top_k]

            hit_clusters = set()
            for idx in top_indices:
                eid = self._entity_ids[idx]
                cid = self._entity_cluster.get(eid, -1)
                if cid != -1:
                    hit_clusters.add(cid)

            results: List[str] = []
            for cid in hit_clusters:
                results.extend(self._clusters.get(cid, []))

            # Deduplicate
            seen: set = set()
            ordered = []
            for eid in results:
                if eid not in seen:
                    seen.add(eid)
                    ordered.append(eid)
            return ordered[:top_k]

    def get_cluster_for(self, entity_id: str) -> int:
        """Return the cluster label for an entity (-1 = noise / unassigned)."""
        return self._entity_cluster.get(entity_id, -1)

    def __len__(self) -> int:
        return len(self._entity_ids)
