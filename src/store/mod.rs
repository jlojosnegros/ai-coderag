use std::{cmp::Ordering, path::PathBuf, sync::Arc};

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::{
    Connection,
    query::{ExecutableQuery, QueryBase},
};

use crate::{Chunk, ChunkId, ChunkMetadata, ChunkStore, CoderagError, Result, ScoredChunk};

pub struct LanceDbStore {
    conn: Connection,
    table_name: String,
    dimension: usize,
    // Cache the schema to avoid rebuilding it on every upsert/query
    schema: Arc<Schema>,
}

mod col {
    pub const ID: &str = "id";
    pub const CONTENT: &str = "content";
    pub const FILE_PATH: &str = "file_path";
    pub const LINE_START: &str = "line_start";
    pub const LINE_END: &str = "line_end";
    pub const LANGUAGE: &str = "language";
    pub const CHUNK_TYPE: &str = "chunk_type";
    pub const SYMBOL_NAME: &str = "symbol_name";
    pub const PARENT_SCOPE: &str = "parent_scope";
    pub const EMBEDDING: &str = "embedding";
    pub const EMBEDDING_ITEM: &str = "item";
    /// Virtual column added by LanceDB to nearest_to() results. Not in the stored schema.
    pub const DISTANCE: &str = "_distance";
}

const TABLE_NAME: &str = "chunks";

impl LanceDbStore {
    /// Open ( or create) the LanceDB database at `path`.
    /// `dimension` must match the embedding model's output dimension exactly.
    pub async fn open(path: &str, dimension: usize) -> Result<Self> {
        let conn = lancedb::connect(path)
            .execute()
            .await
            .map_err(|err| CoderagError::Store(format!("cannot open database at {path}: {err}")))?;

        let schema = Arc::new(Self::build_schema(dimension));

        Ok(Self {
            conn,
            table_name: TABLE_NAME.to_string(),
            dimension,
            schema,
        })
    }

    fn build_schema(dimension: usize) -> Schema {
        Schema::new(vec![
            Field::new(col::ID, DataType::Utf8, false),
            Field::new(col::CONTENT, DataType::Utf8, false),
            Field::new(col::FILE_PATH, DataType::Utf8, false),
            Field::new(col::LINE_START, DataType::UInt32, false),
            Field::new(col::LINE_END, DataType::UInt32, false),
            Field::new(col::LANGUAGE, DataType::Utf8, false),
            Field::new(col::CHUNK_TYPE, DataType::Utf8, false),
            Field::new(col::SYMBOL_NAME, DataType::Utf8, true), // Nullable
            Field::new(col::PARENT_SCOPE, DataType::Utf8, true), // Nullable
            // FixedSizeList stores all embeddings as a flat continuous array
            // each row is a list of exactly `dimension` floats
            // embedding column last (LanceDB works better with the vector column last)
            Field::new(
                col::EMBEDDING,
                DataType::FixedSizeList(
                    Arc::new(Field::new(col::EMBEDDING_ITEM, DataType::Float32, true)),
                    dimension as i32,
                ),
                false,
            ),
        ])
    }

    /// Build and Arrow RecordBatch from a slice of chunks.
    /// All chunks must have embedding = Some(...) with len() == self.dimension
    fn to_record_batch(&self, chunks: &[Chunk]) -> Result<RecordBatch> {
        if chunks.is_empty() {
            return Ok(RecordBatch::new_empty(self.schema.clone()));
        }

        // Validate that all chunks have embeddings of the correct dimension
        for (idx, chunk) in chunks.iter().enumerate() {
            match &chunk.embedding {
                None => {
                    return Err(CoderagError::Store(format!(
                        "chunk {idx} has no embedding. Call embed() before upsert()"
                    )));
                },
                Some(v) if v.len() != self.dimension => {
                    return Err(CoderagError::Store(format!(
                        "chunk {idx} embedding has {} dimensions, expected {}",
                        v.len(),
                        self.dimension
                    )));
                },
                _ => {},
            }
        }

        // Build scalar columns
        let ids = chunks.iter().map(|chunk| chunk.id.0.as_str()).collect::<Vec<_>>();
        let contents = chunks.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>();
        let file_paths = chunks
            .iter()
            .map(|chunk| chunk.metadata.file_path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let file_paths_ref = file_paths.iter().map(String::as_str).collect::<Vec<_>>();
        let line_starts = chunks.iter().map(|chunk| chunk.metadata.line_start).collect::<Vec<_>>();
        let line_ends = chunks.iter().map(|chunk| chunk.metadata.line_end).collect::<Vec<_>>();
        let languages = chunks
            .iter()
            .map(|chunk| chunk.metadata.language.as_str())
            .collect::<Vec<_>>();

        let chunk_types = chunks
            .iter()
            .map(|chunk| chunk.metadata.chunk_type.as_str())
            .collect::<Vec<_>>();

        let symbol_names = chunks
            .iter()
            .map(|chunk| chunk.metadata.symbol_name.as_deref())
            .collect::<Vec<_>>();

        let parent_scopes = chunks
            .iter()
            .map(|chunk| chunk.metadata.parent_scope.as_deref())
            .collect::<Vec<_>>();

        // Build the embedding column
        // flatten all embeddings into a single Vec<f32>, then wrap in FixedSizeListArray
        let flat_embeddings = chunks
            .iter()
            .flat_map(|chunk| chunk.embedding.as_ref().unwrap().iter().copied())
            .collect::<Vec<_>>();
        let values = Arc::new(Float32Array::from(flat_embeddings));
        let embedding_col = Arc::new(FixedSizeListArray::new(
            Arc::new(Field::new(col::EMBEDDING_ITEM, DataType::Float32, true)),
            self.dimension as i32,
            values,
            None,
        )) as Arc<dyn Array>;

        RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as Arc<dyn Array>,
                Arc::new(StringArray::from(contents)) as Arc<dyn Array>,
                Arc::new(StringArray::from(file_paths_ref)) as Arc<dyn Array>,
                Arc::new(UInt32Array::from(line_starts)) as Arc<dyn Array>,
                Arc::new(UInt32Array::from(line_ends)) as Arc<dyn Array>,
                Arc::new(StringArray::from(languages)) as Arc<dyn Array>,
                Arc::new(StringArray::from(chunk_types)) as Arc<dyn Array>,
                Arc::new(StringArray::from(symbol_names)) as Arc<dyn Array>,
                Arc::new(StringArray::from(parent_scopes)) as Arc<dyn Array>,
                embedding_col,
            ],
        )
        .map_err(|err| CoderagError::Store(format!("RecordBatch construction failed: {err}")))
    }

    /// Returns true if the chunks table already exists in this database
    async fn table_exists(&self) -> Result<bool> {
        let names = self
            .conn
            .table_names()
            .execute()
            .await
            .map_err(|err| CoderagError::Store(err.to_string()))?;
        Ok(names.contains(&self.table_name))
    }
}

#[async_trait]
impl ChunkStore for LanceDbStore {
    async fn upsert(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let batch = self.to_record_batch(chunks)?;

        if !self.table_exists().await? {
            // First upsert => Create the table with this batch as initial data.
            // LanceDB infers the schema from the RecordBatch
            self.conn
                .create_table(&self.table_name, batch)
                .execute()
                .await
                .map_err(|err| CoderagError::Store(format!("table creation failed: {err}")))?;
        } else {
            // Table exists: use merge_insert for idempotent upsert keyed on "id"
            let table = self
                .conn
                .open_table(&self.table_name)
                .execute()
                .await
                .map_err(|err| CoderagError::Store(err.to_string()))?;

            let mut op = table.merge_insert(&[col::ID]);
            op.when_matched_update_all(None);
            op.when_not_matched_insert_all();
            op.execute(Box::new(RecordBatchIterator::new(vec![Ok(batch)], self.schema.clone())))
                .await
                .map_err(|err| CoderagError::Store(format!("merge_insert failed: {err}")))?;
        }

        Ok(())
    }

    async fn search_vector(&self, query_vec: &[f32], k: usize) -> Result<Vec<ScoredChunk>> {
        if !self.table_exists().await? {
            return Err(CoderagError::Store(
                "index not found. Run `coderag index <path>` first".to_string(),
            ));
        }

        let table = self
            .conn
            .open_table(&self.table_name)
            .execute()
            .await
            .map_err(|err| CoderagError::Store(err.to_string()))?;

        // nearest_to returns Result<VectorQuery> because it can fail if the
        // embedding column does not exists or the vector has wrong  dimensions.
        let stream = table
            .query()
            .nearest_to(query_vec.to_vec())
            .map_err(|err| CoderagError::Store(format!("nearest_to failed: {err}")))?
            .limit(k)
            .execute()
            .await
            .map_err(|err| CoderagError::Store(err.to_string()))?;

        // Collect all record batches from the result stream
        let batches = stream
            .try_collect::<Vec<_>>()
            .await
            .map_err(|err| CoderagError::Store(err.to_string()))?;

        let mut results = Vec::new();

        for batch in &batches {
            let ids = batch
                .column_by_name(col::ID)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let contents = batch
                .column_by_name(col::CONTENT)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let file_paths = batch
                .column_by_name(col::FILE_PATH)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let line_starts = batch
                .column_by_name(col::LINE_START)
                .unwrap()
                .as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap();

            let line_ends = batch
                .column_by_name(col::LINE_END)
                .unwrap()
                .as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap();

            let languages = batch
                .column_by_name(col::LANGUAGE)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let chunk_types = batch
                .column_by_name(col::CHUNK_TYPE)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let symbol_names = batch
                .column_by_name(col::SYMBOL_NAME)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let parent_scopes = batch
                .column_by_name(col::PARENT_SCOPE)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            // LanceDB appends a "_distance" column to vector search results
            let distances = batch
                .column_by_name(col::DISTANCE)
                .unwrap()
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap();

            for idx in 0..batch.num_rows() {
                let l2_distance = distances.value(idx);
                // Convert L2 distance to cosine similarity score.
                // For unit-norm vectors
                // L2 = sqrt(2(1 - cos))
                // Max L2 = 2.0 ( meaning oposite vectors)
                // score is in [0.0, 1.0] where:
                // 1.0 => means identical
                // 0.0 => means oposite
                let score = 1.0 - (l2_distance / 2.0);

                results.push(ScoredChunk {
                    chunk: Chunk {
                        id: ChunkId(ids.value(idx).to_string()),
                        content: contents.value(idx).to_string(),
                        metadata: ChunkMetadata {
                            file_path: PathBuf::from(file_paths.value(idx)),
                            line_start: line_starts.value(idx),
                            line_end: line_ends.value(idx),
                            language: languages.value(idx).parse().unwrap(),
                            chunk_type: chunk_types.value(idx).parse().unwrap(),
                            symbol_name: if symbol_names.is_null(idx) {
                                None
                            } else {
                                Some(symbol_names.value(idx).to_string())
                            },
                            parent_scope: if parent_scopes.is_null(idx) {
                                None
                            } else {
                                Some(parent_scopes.value(idx).to_string())
                            },
                        },
                        embedding: None,
                    },
                    score,
                });
            }
        }

        // Sort by descending score ( LanceDB returns by ascending distance,
        // but multiple batches may not be globally sorted. Ensure consistent order)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        Ok(results)
    }
}
