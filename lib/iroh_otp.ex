defmodule IrohEx.NodeManager do
  use GenServer
  alias IrohEx.Native
  alias IrohEx.NodeConfig
  require Logger

  @default_log_collector IrohEx.LogCollector
  # @delay_after_batch 500

  def start_link(log_collector \\ @default_log_collector) do

    IO.inspect(log_collector, label: "Log collector impl")

    case GenServer.start_link(__MODULE__, log_collector) do
      # case Sensocto.RegistryUtils.dynamic_start_child(Sensocto.SensorsDynamicSupervisor, __MODULE__, child_spec) do
      {:ok, pid} when is_pid(pid) ->
        Logger.debug("#{__MODULE__} started")
        {:ok, pid}

      {:error, {:already_started, _pid}} ->
        Logger.debug("#{__MODULE__} already started")
        {:ok, :already_started}

      {:error, reason} ->
        Logger.debug("Error starting #{__MODULE__}, reason: #{reason}")
        {:error, reason}
    end
  end

  @impl true
  def init(log_pid) do
    children = [
      # log_collector,
      # log_collector,
      {IrohEx.NodeSupervisor, []}
    ]

    IO.inspect(children, label: "Childspec")
    # IO.inspect(ticket, label: "init Ticket")

    opts = [strategy: :one_for_one, name: Iroh.Supervisor]

    { :ok, sup } = case Supervisor.start_link(children, opts) do
      # case Sensocto.RegistryUtils.dynamic_start_child(Sensocto.SensorsDynamicSupervisor, __MODULE__, child_spec) do
      {:ok, pid} when is_pid(pid) ->
        Logger.debug("#{__MODULE__} Supervisor started")
        {:ok, pid}

      {:error, {:already_started, _pid}} ->
        Logger.debug("#{__MODULE__} Supervisor already started")
        {:ok, :already_started}

      {:error, reason} ->
        Logger.debug("Error starting #{__MODULE__} Supervisor, reason: #{reason}")
        {:error, reason}
    end

    {:ok, %{ sup: sup, log_pid: log_pid}}
  end

  @impl true
  def handle_cast({:send_message, pid, payload}, state) do
    #GenServer.cast(pid, {:send_message, payload })
    Process.send_after(pid, {:send_message, payload }, 0)
    {:noreply, state}
  end

  @impl true
  def handle_call(:get_random_node, _from, state) do
    node = IrohEx.NodeSupervisor.get_children()
    |> Enum.random()
    {:reply, {:ok, node}, state}
  end

  @impl true
  def handle_call({:create_nodes, whale_node_prob, node_count, log_pid, delay_after_batch}, _from, state) do

    # kill all existing nodes
    IrohEx.NodeSupervisor.reset()

    node_config = NodeConfig.build()
      |> Map.put(:active_view_capacity, 50)
      |> Map.put(:passive_view_capacity, 200)

    mothership_node_ref = Native.create_node(self(), node_config)
    ticket = Native.create_ticket(mothership_node_ref)

    # IO.inspect(state, label: "State")
    # 1..node_count
    # |> Enum.map(fn _ ->
    #   Task.async(fn ->
    #     IO.puts("Start node")
    #     #Process.send_after(pid, :create_node, 0)
    #     Task.async(fn -> IrohEx.NodeSupervisor.start_node(ticket, log_pid) end)
    #   end)
    # end)

    Process.send_after(self(), {:create_nodes_batch, whale_node_prob, delay_after_batch}, 0)

    #|> Enum.map(&Task.await/1)
    {:reply, IrohEx.NodeSupervisor.get_children(), state
      |> Map.put(:nodes_to_create, node_count)
      |> Map.put(:mothership_node_ref, mothership_node_ref)
      |> Map.put(:ticket, ticket)
      |> Map.put(:log_pid, log_pid)
    }
  end

  @impl true
  def handle_call(:reset, _from, state) do
    # kill all existing nodes
    IrohEx.NodeSupervisor.reset()

    {:reply, [], state
      |> Map.put(:nodes_to_create, 0)
      |> Map.put(:mothership_node_ref, nil)
      |> Map.put(:ticket, nil)
    }
  end

  @impl true
  def handle_info({:create_nodes_batch, whale_node_prob, delay_ms}, %{nodes_to_create: nodes_to_create, ticket: ticket, log_pid: log_pid} = state) do
    max_concurrency = System.schedulers_online()

    IO.puts("#{__MODULE__} handle_info :create_nodes_batch #{whale_node_prob} #{delay_ms} #{nodes_to_create}")
    batch_size = min(max_concurrency, nodes_to_create)

    stream =
      1..batch_size
      |> Stream.map(fn _ ->
        fn ->
          # N% chance
          is_whale_node = case whale_node_prob > 0 do
            true -> :rand.uniform() <= max(1, whale_node_prob) / 100
            false -> false
          end

          # IO.puts("#{__MODULE__} Node: #{x} is whale: #{is_whale_node}")

          IrohEx.NodeSupervisor.start_node(is_whale_node, ticket, log_pid)
          # IO.puts("Node #{x} started")
          :ok
        end
      end)

    results =
      stream
      |> Task.async_stream(fn action -> action.() end, max_concurrency: max_concurrency)
      |> Enum.to_list()

    started_nodes = Enum.count(results, fn
      {:ok, :ok} -> true
      _ -> false
    end)

    IO.puts("#{__MODULE__} Started #{started_nodes} nodes in batch")

    remaining = nodes_to_create - started_nodes

    if remaining > 0 do
      Process.send_after(self(), {:create_nodes_batch, whale_node_prob, delay_ms}, delay_ms)
    end

    {:noreply, state
      |> Map.put(:nodes_to_create, remaining)
    }
  end

  @impl true
  def handle_info(_msg, state) do
    # IO.inspect(msg, label: "#{__MODULE__} handle_info catchall #{inspect(msg)}")
    {:noreply, state}
  end

end

defmodule IrohEx.NodeSupervisor do
  use DynamicSupervisor
  require Logger

  def start_link(args) do
    DynamicSupervisor.start_link(__MODULE__, args, name: __MODULE__)
  end

  @impl true
  def init(_args) do
    DynamicSupervisor.init(
      strategy: :one_for_one,
      max_restarts: 1000,        # Even higher
      max_seconds: 600,          # Longer window
      max_children: :infinity,
      intensity: 1,
      period: 1,
      # Add these to help debug supervisor behavior
      trap_exit: true,           # Trap exits to prevent supervisor crashes
      shutdown: :brutal_kill     # Force kill children that don't terminate
    )
  end

  def start_node(is_whale_node, ticket, log_pid) do
    node_id = "Node:#{:rand.uniform(1_000_000)}"
    child_spec = %{
      id: node_id,
      start: {IrohEx.Node, :start_link, [%{is_whale_node: is_whale_node, ticket: ticket, log_pid: log_pid}]},
      shutdown: 5_000,
      restart: :permanent,     # Always restart
      type: :worker
    }

    case DynamicSupervisor.start_child(__MODULE__, child_spec) do
      {:ok, pid} when is_pid(pid) ->
        Logger.debug("Added node #{node_id}")
        {:ok, pid}

      {:error, {:already_started, _pid}} ->
        Logger.debug("Node already started #{node_id}")
        {:ok, :already_started}

      {:error, reason} ->
        Logger.debug("error adding node #{node_id}: #{inspect(reason)}")
        {:error, reason}
    end
  end

  def check_health do
    children = DynamicSupervisor.which_children(__MODULE__)
    Logger.debug("Supervisor health check:")
    Logger.debug("  Process: #{inspect(Process.whereis(__MODULE__))}")
    Logger.debug("  Children count: #{length(children)}")
    Logger.debug("  Children: #{inspect(children)}")
    children
  end

  def get_children do
    DynamicSupervisor.which_children(__MODULE__)
    |> Enum.map(fn {_, pid, _, _} -> pid end)
  end

  def reset() do
    DynamicSupervisor.which_children(__MODULE__)
    |> Enum.each(fn {_, pid, _, _} -> DynamicSupervisor.terminate_child(__MODULE__, pid) end)
  end

  def debug_state do
    children = DynamicSupervisor.which_children(__MODULE__)
    IO.puts("Supervisor children: #{inspect(children)}")
    # Also check if supervisor is alive
    case Process.whereis(__MODULE__) do
      nil -> IO.puts("Supervisor process not found!")
      pid -> IO.puts("Supervisor process: #{inspect(pid)}")
    end
    children
  end
end


defmodule IrohEx.Node do
  use GenServer
  require Logger
  alias IrohEx.Native
  alias IrohEx.NodeConfig
  def start_link(state) do
    GenServer.start_link(__MODULE__, state)
  end

  @impl true
  def init(%{ticket: ticket, log_pid: log_pid, is_whale_node: is_whale_node} = _state) do
    # Process.flag(:trap_exit, true)
    IO.puts("#{__MODULE__} init")
    Process.send_after(self(), :setup_node, 0)
    {:ok, %{ticket: ticket, is_whale_node: is_whale_node, log_pid: log_pid, peers: [], messages_sent: [], messages_received: []}}
  end

  @impl true
  def handle_info(:setup_node, %{is_whale_node: is_whale_node, ticket: _ticket, log_pid: log_pid } = state) do
    # IO.puts("setup_node ticket: #{inspect(ticket)}, log_pid: #{inspect(log_pid)}")
    #IO.inspect(state, label: "Node init state")
    pid = self()

    node_config = case is_whale_node do
      true ->
        NodeConfig.build()
        |> Map.put(:is_whale_node, true)
        |> Map.put(:active_view_capacity, 50)
        |> Map.put(:passive_view_capacity, 200)
      false ->
        NodeConfig.build()
        |> Map.put(:is_whale_node, false)
        |> Map.put(:active_view_capacity, 0)
        |> Map.put(:passive_view_capacity, 0)
    end

    node_ref = Native.create_node(pid, node_config)
    # tiny nap to let p2p do it's thing
    Process.sleep(500)
    node_addr = Native.gen_node_addr(node_ref)


    # send immediately, otherwise all :continue callbacks will need to finish before
    Process.send_after(log_pid, {:iroh_node_setup, node_addr}, 0)
    Process.send_after(self(), :connect_node, 0)

    #IO.inspect(node_addr, "Node init")
    {:noreply, state
      |> Map.put(:node_ref, node_ref)
      |> Map.put(:node_addr, node_addr),
      # {:continue, :connect_node}
    }
  end

  def handle_info(:connect_node, %{node_ref: node_ref, ticket: ticket, log_pid: _log_pid } = state) do
    # IO.puts("connect_node Node addr: #{inspect(node_addr)}")
    Native.connect_node(node_ref, ticket)
    Process.sleep(500)
    #IO.inspect(node_addr, "Node init")
    {:noreply, state}
  end

  def handle_info({:send_message, payload }, %{ node_ref: node_ref, node_addr: _node_addr, messages_sent: messages_sent } = state) do
    # IO.puts("#{__MODULE__} handle_info send_message #{node_addr} #{payload}")
    # IO.inspect(state, "handle_info send_message state")
    timestamp = DateTime.utc_now() |> DateTime.to_unix(:millisecond)
    Native.send_message(node_ref, payload)
    {:noreply, state
      |> Map.put(:messages_sent, messages_sent ++ [%{timestamp: timestamp, payload: payload}])
    }
  end

  # IrohEx.Node.handle_cast({:send_message, "Test manager"},
  # def handle_info({:send_message, payload }, %{node_ref: node_ref, log_pid: log_pid, messages_sent: messages_sent} = state) do
  #   timestamp = DateTime.utc_now() |> DateTime.to_unix(:millisecond)
  #   Native.send_message(node_ref, payload)
  #   {:noreply, %{state | messages_sent: messages_sent ++ [%{timestamp: timestamp, payload: payload}]}}
  # end

  @impl true
  def handle_info({:iroh_gossip_message_received, source, msg}, %{ log_pid: log_pid, messages_received: messages_received } = state) do
    # IO.puts("#{__MODULE__} handle_info iroh_gossip_message_received PID alive?: #{Process.alive?(log_pid)} source: #{source} msg: #{msg} #{inspect(Enum.count(messages_received))}")
    timestamp = DateTime.utc_now() |> DateTime.to_unix(:millisecond)

    Process.send_after(log_pid, {:iroh_gossip_message_received, source, msg}, 0)

    {:noreply, state
    |> Map.put(:messages_received, messages_received ++ [%{timestamp: timestamp, payload: msg}])
  }
  end

  def handle_info(msg, %{log_pid: log_pid} = state) do
    # IO.inspect(msg, label: "#{__MODULE__} handle_info catchall #{inspect(msg)}")
    Process.send_after(log_pid, msg, 0)
    {:noreply, state}
  end

  def handle_info(msg, %{ node_ref: _node_ref, node_addr: node_addr } = state) do
    IO.puts("#{__MODULE__} Catchall #{node_addr} #{inspect(msg)}")
    {:noreply, state}
  end

  @impl true
  def handle_call(:get_node_addr, _from, %{node_ref: node_ref } = state) do
    node_addr = Native.gen_node_addr(node_ref)
    {:reply, node_addr, state}
  end

  @impl true
  def handle_call(:report, _from, %{node_addr: _node_addr, peers: peers, messages_sent: messages_sent, messages_received: messages_received} = state) do

    # IO.puts("Hello there")

    # IO.inspect(state, label: "#{__MODULE__} #{node_addr}")

    {:reply, %{
      peers: peers,
      messages_sent_cnt: Enum.count(messages_sent),
      messages_received_cnt: Enum.count(messages_received),
      messages_sent: messages_sent,
      messages_received: messages_received
    }, state}
  end

  @impl true
  def terminate(reason, %{ log_pid: log_pid, node_ref: node_ref, node_addr: node_addr } = state) do
    Logger.debug("Node terminating: #{inspect(reason)}")
    if node_ref do
      try do
        Logger.debug("Cleaning up node_ref: #{inspect(node_ref)}")
        result = Native.cleanup(node_ref)
        Logger.debug("Cleanup result: #{inspect(result)}")
      rescue
        e -> Logger.error("Error cleaning up node: #{inspect(e)}")
      end
    else
      Logger.debug("No node_ref to clean up")
    end

    :ok
  end


end

defmodule IrohEx.LogCollector do
  use GenServer

  def init(init_arg) do
    {:ok, init_arg}
  end

  def start_link(_args) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  def handle_info(_msg, state) do
    # IO.inspect(msg, label: "#{__MODULE__} handle_info catchall #{inspect(msg)}")
    {:noreply, state}
  end
end
