defmodule IrohEx.Automerge do
  @moduledoc """
  High-level API for automerge CRDT documents.

  Automerge is a JSON-like data structure (a CRDT) that can be modified
  concurrently by different users, and merged again automatically.

  ## Features

  - **Maps**: Nested key-value structures
  - **Lists**: Ordered sequences with concurrent insert support
  - **Text**: Collaborative text editing with character-level merging
  - **Counters**: Distributed counters that always converge correctly

  ## Basic Usage

      # Create a node first
      node = IrohEx.Native.create_node(self(), IrohEx.NodeConfig.build())

      # Create a document
      doc_id = IrohEx.Automerge.new(node)

      # Put some data
      :ok = IrohEx.Automerge.put(node, doc_id, [], "name", "Alice")
      :ok = IrohEx.Automerge.put(node, doc_id, [], "age", 30)

      # Create nested structures
      IrohEx.Automerge.create_map(node, doc_id, [], "profile")
      :ok = IrohEx.Automerge.put(node, doc_id, ["profile"], "email", "alice@example.com")

      # Read data
      "Alice" = IrohEx.Automerge.get(node, doc_id, [], "name")

  ## Merging Documents

      # Fork a document to simulate concurrent edits
      doc2 = IrohEx.Automerge.fork(node, doc_id)

      # Make changes to both
      :ok = IrohEx.Automerge.put(node, doc_id, [], "field_a", "value_a")
      :ok = IrohEx.Automerge.put(node, doc2, [], "field_b", "value_b")

      # Merge doc2 into doc_id
      {:ok, bytes} = IrohEx.Automerge.save(node, doc2)
      :ok = IrohEx.Automerge.merge(node, doc_id, bytes)

      # Now doc_id has both changes!
  """

  alias IrohEx.Native

  @type doc_id :: binary()
  @type path :: [binary()]
  @type node_ref :: reference()

  # ============================================================================
  # Document Lifecycle
  # ============================================================================

  @doc """
  Create a new empty automerge document.

  ## Examples

      doc_id = IrohEx.Automerge.new(node)
  """
  @spec new(node_ref()) :: doc_id()
  def new(node), do: Native.automerge_create_doc(node)

  @doc """
  Fork a document (create a copy with shared history).

  Forking is useful for:
  - Simulating concurrent edits in tests
  - Creating branches of a document
  - Offline editing that will be merged later

  ## Examples

      doc2 = IrohEx.Automerge.fork(node, doc_id)
  """
  @spec fork(node_ref(), doc_id()) :: doc_id()
  def fork(node, doc_id), do: Native.automerge_fork_doc(node, doc_id)

  @doc """
  Load a document from binary data.

  ## Examples

      {:ok, doc_id} = IrohEx.Automerge.load(node, saved_bytes)
  """
  @spec load(node_ref(), binary()) :: {:ok, doc_id()} | {:error, term()}
  def load(node, data) do
    {:ok, Native.automerge_load_doc(node, data)}
  rescue
    e -> {:error, e}
  end

  @doc """
  Save a document to binary format.

  The binary format is compact and can be stored or transmitted.

  ## Examples

      {:ok, bytes} = IrohEx.Automerge.save(node, doc_id)
      File.write!("document.automerge", bytes)
  """
  @spec save(node_ref(), doc_id()) :: {:ok, binary()} | {:error, term()}
  def save(node, doc_id) do
    {:ok, Native.automerge_save_doc(node, doc_id)}
  rescue
    e -> {:error, e}
  end

  @doc """
  Delete a document from memory.

  ## Examples

      true = IrohEx.Automerge.delete(node, doc_id)
  """
  @spec delete(node_ref(), doc_id()) :: boolean()
  def delete(node, doc_id), do: Native.automerge_delete_doc(node, doc_id)

  @doc """
  List all document IDs.

  ## Examples

      doc_ids = IrohEx.Automerge.list(node)
  """
  @spec list(node_ref()) :: [doc_id()]
  def list(node), do: Native.automerge_list_docs(node)

  # ============================================================================
  # Map Operations
  # ============================================================================

  @doc """
  Put a value at a nested path.

  ## Parameters

  - `node` - The node reference
  - `doc_id` - The document ID
  - `path` - List of keys to navigate to (empty for root)
  - `key` - The key to set
  - `value` - The value (string, integer, float, boolean, or binary)

  ## Examples

      # Set at root level
      :ok = IrohEx.Automerge.put(node, doc_id, [], "name", "Alice")

      # Set at nested path (path must exist)
      :ok = IrohEx.Automerge.put(node, doc_id, ["users", "alice"], "email", "alice@example.com")
  """
  @spec put(node_ref(), doc_id(), path(), binary(), term()) :: :ok | {:error, term()}
  def put(node, doc_id, path, key, value) do
    Native.automerge_map_put(node, doc_id, path, key, value)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get a value at a nested path.

  ## Examples

      "Alice" = IrohEx.Automerge.get(node, doc_id, [], "name")
      :not_found = IrohEx.Automerge.get(node, doc_id, [], "nonexistent")
  """
  @spec get(node_ref(), doc_id(), path(), binary()) :: term()
  def get(node, doc_id, path, key) do
    Native.automerge_map_get(node, doc_id, path, key)
  end

  @doc """
  Create a nested map structure.

  ## Examples

      IrohEx.Automerge.create_map(node, doc_id, [], "users")
      IrohEx.Automerge.create_map(node, doc_id, ["users"], "alice")
      :ok = IrohEx.Automerge.put(node, doc_id, ["users", "alice"], "name", "Alice")
  """
  @spec create_map(node_ref(), doc_id(), path(), binary()) :: {:ok, binary()} | {:error, term()}
  def create_map(node, doc_id, path, key) do
    {:ok, Native.automerge_map_put_object(node, doc_id, path, key, "map")}
  rescue
    e -> {:error, e}
  end

  @doc """
  Delete a key from a map.

  ## Examples

      :ok = IrohEx.Automerge.delete_key(node, doc_id, [], "old_field")
  """
  @spec delete_key(node_ref(), doc_id(), path(), binary()) :: :ok | {:error, term()}
  def delete_key(node, doc_id, path, key) do
    Native.automerge_map_delete(node, doc_id, path, key)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get all keys from a map.

  ## Examples

      ["name", "age", "email"] = IrohEx.Automerge.keys(node, doc_id, [])
  """
  @spec keys(node_ref(), doc_id(), path()) :: [binary()]
  def keys(node, doc_id, path) do
    Native.automerge_map_keys(node, doc_id, path)
  end

  # ============================================================================
  # List Operations
  # ============================================================================

  @doc """
  Create a list at the given path.

  ## Examples

      IrohEx.Automerge.create_list(node, doc_id, [], "items")
  """
  @spec create_list(node_ref(), doc_id(), path(), binary()) :: {:ok, binary()} | {:error, term()}
  def create_list(node, doc_id, path, key) do
    {:ok, Native.automerge_map_put_object(node, doc_id, path, key, "list")}
  rescue
    e -> {:error, e}
  end

  @doc """
  Push a value to the end of a list.

  ## Examples

      IrohEx.Automerge.create_list(node, doc_id, [], "items")
      :ok = IrohEx.Automerge.list_push(node, doc_id, ["items"], "first")
      :ok = IrohEx.Automerge.list_push(node, doc_id, ["items"], "second")
  """
  @spec list_push(node_ref(), doc_id(), path(), term()) :: :ok | {:error, term()}
  def list_push(node, doc_id, path, value) do
    Native.automerge_list_push(node, doc_id, path, value)
  rescue
    e -> {:error, e}
  end

  @doc """
  Insert a value at a specific index.

  ## Examples

      :ok = IrohEx.Automerge.list_insert(node, doc_id, ["items"], 0, "at_beginning")
  """
  @spec list_insert(node_ref(), doc_id(), path(), non_neg_integer(), term()) :: :ok | {:error, term()}
  def list_insert(node, doc_id, path, index, value) do
    Native.automerge_list_insert(node, doc_id, path, index, value)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get a value from a list by index.

  ## Examples

      "first" = IrohEx.Automerge.list_get(node, doc_id, ["items"], 0)
  """
  @spec list_get(node_ref(), doc_id(), path(), non_neg_integer()) :: term()
  def list_get(node, doc_id, path, index) do
    Native.automerge_list_get(node, doc_id, path, index)
  end

  @doc """
  Delete a value from a list by index.

  ## Examples

      :ok = IrohEx.Automerge.list_delete(node, doc_id, ["items"], 0)
  """
  @spec list_delete(node_ref(), doc_id(), path(), non_neg_integer()) :: :ok | {:error, term()}
  def list_delete(node, doc_id, path, index) do
    Native.automerge_list_delete(node, doc_id, path, index)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get the length of a list.

  ## Examples

      3 = IrohEx.Automerge.list_length(node, doc_id, ["items"])
  """
  @spec list_length(node_ref(), doc_id(), path()) :: non_neg_integer()
  def list_length(node, doc_id, path) do
    Native.automerge_list_length(node, doc_id, path)
  end

  # ============================================================================
  # Text Operations
  # ============================================================================

  @doc """
  Create a collaborative text object.

  Text objects support character-level concurrent editing with automatic
  conflict resolution.

  ## Examples

      IrohEx.Automerge.create_text(node, doc_id, [], "content", "Hello World")
  """
  @spec create_text(node_ref(), doc_id(), path(), binary(), binary()) :: {:ok, binary()} | {:error, term()}
  def create_text(node, doc_id, path, key, initial_text \\ "") do
    {:ok, Native.automerge_text_create(node, doc_id, path, key, initial_text)}
  rescue
    e -> {:error, e}
  end

  @doc """
  Insert text at a position.

  ## Examples

      :ok = IrohEx.Automerge.text_insert(node, doc_id, ["content"], 5, " Beautiful")
      # "Hello Beautiful World"
  """
  @spec text_insert(node_ref(), doc_id(), path(), non_neg_integer(), binary()) :: :ok | {:error, term()}
  def text_insert(node, doc_id, path, position, text) do
    Native.automerge_text_insert(node, doc_id, path, position, text)
  rescue
    e -> {:error, e}
  end

  @doc """
  Delete text at a position.

  ## Examples

      :ok = IrohEx.Automerge.text_delete(node, doc_id, ["content"], 5, 10)
      # Deletes 10 characters starting at position 5
  """
  @spec text_delete(node_ref(), doc_id(), path(), non_neg_integer(), non_neg_integer()) :: :ok | {:error, term()}
  def text_delete(node, doc_id, path, position, length) do
    Native.automerge_text_delete(node, doc_id, path, position, length)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get the full text content.

  ## Examples

      "Hello World" = IrohEx.Automerge.text_get(node, doc_id, ["content"])
  """
  @spec text_get(node_ref(), doc_id(), path()) :: binary()
  def text_get(node, doc_id, path) do
    Native.automerge_text_get(node, doc_id, path)
  end

  # ============================================================================
  # Counter Operations
  # ============================================================================

  @doc """
  Increment a counter (creates it if it doesn't exist).

  Counters are a special CRDT type that always converge correctly,
  even with concurrent increments from multiple nodes.

  ## Examples

      1 = IrohEx.Automerge.counter_increment(node, doc_id, [], "views", 1)
      6 = IrohEx.Automerge.counter_increment(node, doc_id, [], "views", 5)
      4 = IrohEx.Automerge.counter_increment(node, doc_id, [], "views", -2)
  """
  @spec counter_increment(node_ref(), doc_id(), path(), binary(), integer()) :: integer()
  def counter_increment(node, doc_id, path, key, delta) do
    Native.automerge_counter_increment(node, doc_id, path, key, delta)
  end

  @doc """
  Get the current counter value.

  ## Examples

      42 = IrohEx.Automerge.counter_get(node, doc_id, [], "views")
  """
  @spec counter_get(node_ref(), doc_id(), path(), binary()) :: integer()
  def counter_get(node, doc_id, path, key) do
    Native.automerge_counter_get(node, doc_id, path, key)
  end

  # ============================================================================
  # Sync Operations
  # ============================================================================

  @doc """
  Merge another document's changes into this one.

  ## Examples

      {:ok, other_bytes} = IrohEx.Automerge.save(node, other_doc_id)
      :ok = IrohEx.Automerge.merge(node, doc_id, other_bytes)
  """
  @spec merge(node_ref(), doc_id(), binary()) :: :ok | {:error, term()}
  def merge(node, doc_id, other_doc_bytes) do
    Native.automerge_merge(node, doc_id, other_doc_bytes)
  rescue
    e -> {:error, e}
  end

  @doc """
  Sync document with all connected peers via gossip.

  This broadcasts the full document to all peers in the gossip network.

  ## Examples

      :ok = IrohEx.Automerge.sync(node, doc_id)
  """
  @spec sync(node_ref(), doc_id()) :: :ok | {:error, term()}
  def sync(node, doc_id) do
    Native.automerge_sync_via_gossip(node, doc_id)
  rescue
    e -> {:error, e}
  end

  @doc """
  Get document as JSON string for debugging.

  ## Examples

      json = IrohEx.Automerge.to_json(node, doc_id)
      IO.puts(json)
  """
  @spec to_json(node_ref(), doc_id()) :: binary()
  def to_json(node, doc_id) do
    Native.automerge_to_json(node, doc_id)
  end
end
