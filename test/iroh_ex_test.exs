defmodule IrohExTest do
  use ExUnit.Case
  doctest IrohEx
  alias IrohEx.Native

  @node_cnt 20
  # 10_000
  @msg_cnt 10
  @rand_msg_delay 5000

  @msg_timeout 10_000

  test "test iroh node" do
    node_ref = Native.create_node(self())
    ticket = Native.create_ticket(node_ref)

    node_id = Native.gen_node_addr(node_ref)

    _node_ref_connected = Native.connect_node(node_ref, ticket)

    IO.inspect(node_id, label: "Node id")
    assert is_binary(node_id)
  end

  test "test iroh node messages" do
    node_ref = Native.create_node(self())
    ticket = Native.create_ticket(node_ref)

    node_id = Native.gen_node_addr(node_ref)
    node2_ref = Native.create_node(self())

    _node_ref_connected = Native.connect_node(node_ref, ticket)
    _node2_ref_connected = Native.connect_node(node2_ref, ticket)

    Process.sleep(2000)

    receive do
      {:iroh_gossip_node_discovered, node_source, node_discovered} ->
        IO.puts("Node discovered: #{node_source}, #{node_discovered}")
        :ok
        # refute_receive {:btleplug_scan_stopped, _msg}
    after
      @msg_timeout -> flunk("Did not receive :iroh_gossip_node_discovered message")
    end

    # assert_receive {:iroh_gossip_node_discovered, node_source, node_discovered}

    Native.send_message(node_ref, "Test message")

    receive do
      {:iroh_gossip_message_received, node_source, msg} ->
        IO.puts("Message received: #{node_source} #{msg}")
        :ok
        # refute_receive {:btleplug_scan_stopped, _msg}
    after
      @msg_timeout -> flunk("Did not receive :iroh_gossip_message_received message")
    end

    # assert_receive {:iroh_gossip_message_received, node_source, msg}

    assert is_binary(node_id)
  end

  test "test many iroh nodes" do
    mothership_node_ref = Native.create_node(self())

    ticket = Native.create_ticket(mothership_node_ref)

    IO.inspect(ticket, label: "Node1 ticket")

    # connect main node
    # Task.async(fn -> Native.connect_node(mothership_node_ref, ticket) end)
    Task.async(fn -> Native.connect_node(mothership_node_ref, ticket) end)

    nodes_cnt =
      case Integer.parse(System.get_env("NODES_CNT", "#{@node_cnt}")) do
        {nodes_cnt, _} -> nodes_cnt
        _ -> @node_cnt
      end

    nodes = create_nodes(nodes_cnt, self())

    IO.inspect(nodes, label: "Node list")

    # nodes =
    #   Enum.map(1..count, fn _ ->
    #     Native.create_node(self())
    #   end)

    tasks =
      Enum.map(nodes, fn n ->
        IO.inspect(n, label: "Connect Node ref")
        Task.async(fn -> Native.connect_node(n, ticket) end)
      end)

    Enum.each(tasks, &Task.await/1)

    # Process.sleep(2000)
    IO.puts("starting msg loop")

    msg_cnt =
      case Integer.parse(System.get_env("MSG_CNT", "#{@msg_cnt}")) do
        {msg_cnt, _} -> msg_cnt
        _ -> @msg_cnt
      end

    tasks =
      Enum.map(1..msg_cnt, fn x ->
        Task.async(fn ->
          # IO.puts("Nodes: #{Enum.count(nodes)}")
          # node = Enum.at(nodes, :rand.uniform(Enum.count(nodes) - 1))
          node = Enum.random(nodes)
          # node = mothership_node_ref
          _node_id = Native.gen_node_addr(node)
          # IO.inspect(node, label: "Send msg Node ref")

          rand_msg_delay =
            case Integer.parse(System.get_env("RAND_MSG_DELAY", "#{@rand_msg_delay}")) do
              {rand_msg_delay, _} -> rand_msg_delay
              _ -> @rand_msg_delay
            end

          # Process.sleep(:rand.uniform(rand_msg_delay))
          Process.sleep(200)
          # from #{node_id}
          Native.send_message(node, "MSG:#{x}")
        end)
      end)

    IO.inspect(Enum.count(tasks), label: "Tasks")

    Enum.each(tasks, &Task.await/1)

    Process.sleep(5000)

    # msg_counts = count_messages()
    # IO.inspect(msg_counts, label: "Messages")

    {:messages, messages} = :erlang.process_info(self(), :messages)

    # Write each message to a file correctly
    File.open!("erlang_mailbox_dump.txt", [:write], fn file ->
      Enum.each(messages, fn msg ->
        IO.write(file, "#{inspect(msg, pretty: true)}\n")
      end)
    end)

    nodes_parsed = GossipParser.parse_gossip_messages(messages)

    File.open!("gossip_nodes_dump.txt", [:write], fn file ->
      Enum.each(nodes_parsed.nodes, fn msg ->
        IO.write(file, "#{inspect(msg, pretty: true)}\n")
      end)
    end)

    # mermaid_viz = MermaidGenerator.generate_mermaid_graph(nodes_parsed)
    mermaid_viz = MermaidGenerator.generate_mermaid_graph(nodes_parsed)

    File.open!("gossip_nodes_mermaid.mmd", [:write], fn file ->
      IO.write(file, "#{mermaid_viz}\n")
    end)

    IO.inspect(nodes_parsed, label: "Parsed gossip message")
  end

  def create_nodes(node_count, pid) when is_integer(node_count) and node_count > 0 do
    1..node_count
    |> Enum.map(fn _ ->
      Task.async(fn -> Native.create_node_async(pid) end)
    end)
    # Await results
    |> Enum.map(&Task.await/1)
    |> Enum.reduce([], fn node_ref, acc ->
      case node_ref do
        # Collect valid references
        ref when is_reference(ref) ->
          [ref | acc]

        error ->
          IO.puts("Error creating node: #{inspect(error)}")
          # Skip errors
          acc
      end
    end)
    # |> dbg()
    # Maintain original order
    |> Enum.reverse()
  end

  defp count_messages(acc \\ %{received: 0, discovered: 0, other: 0}, timeout \\ 500) do
    receive do
      {:iroh_gossip_message_received, _node_source, _msg} ->
        count_messages(%{acc | received: acc.received + 1}, timeout)

      {:iroh_gossip_node_discovered, _node_source, _node_discovered} ->
        count_messages(%{acc | discovered: acc.discovered + 1}, timeout)

      other_event ->
        IO.puts("Other event #{inspect(other_event)}")
        count_messages(%{acc | other: acc.other + 1}, timeout)
    after
      # Stop after a longer timeout
      timeout -> acc
    end
  end
end

defmodule GossipParser do
  def parse_gossip_messages(messages) do
    Enum.reduce(messages, %{nodes: %{}}, fn
      {:iroh_gossip_node_discovered, source, discovered}, acc ->
        update_in(acc, [:nodes, source], fn node ->
          case node do
            # Initialize node with peers
            nil ->
              %{peers: [discovered]}

            node ->
              update_in(node, [:peers], fn
                nil -> [discovered]
                peers -> [discovered | peers]
              end)
          end
        end)

      {:iroh_gossip_message_received, source, msg}, acc ->
        update_in(acc, [:nodes, source], fn node ->
          case node do
            # Initialize node with messages
            nil ->
              %{messages: [msg]}

            node ->
              update_in(node, [:messages], fn
                nil -> [msg]
                msgs -> [msg | msgs]
              end)
          end
        end)

      # Ignore other messages
      _other, acc ->
        acc
    end)
  end

  def map_put(data, keys, value) do
    # data = %{} or non empty map
    # keys = [:a, :b, :c]
    # value = 3
    put_in(data, Enum.map(keys, &Access.key(&1, %{})), value)
  end

  def many_map_puts(data, keys_values) do
    # data = %{} or non empty map
    # keys_values = [[keys: [:a, :b, :c], value: 4],[keys: [:z, :y, :x], value: 90]]
    Enum.reduce(keys_values, data, fn x, data ->
      map_put(data, x[:keys], x[:value])
    end)
  end
end

defmodule MermaidGenerator do
  def generate_mermaid_graph(node_data) do
    nodes_string =
      node_data.nodes
      |> Enum.map(fn {node_id, node_info} ->
        # Default to empty list if missing
        messages = Map.get(node_info, :messages, [])

        message_string =
          messages
          |> Enum.with_index()
          # Concise msg info
          |> Enum.map(fn {msg, index} -> "M#{index + 1}: #{msg}" end)
          |> Enum.join("<br>")

        """
        #{node_id}("#{node_id}<br>Msgs: #{Enum.count(messages)}<br>#{message_string}"):::node
        """
      end)
      |> Enum.join("\n")

    connections_string =
      node_data.nodes
      |> Enum.flat_map(fn {source_id, source_info} ->
        Enum.map(source_info.peers, fn target_id ->
          """
          #{source_id} --> #{target_id}
          """
        end)
      end)
      # Remove duplicate connections
      |> Enum.uniq()
      |> Enum.join("\n")

    style_string =
      node_data.nodes
      |> Enum.with_index()
      |> Enum.map(fn {{node_id, _node_info}, index} ->
        colors = [
          "#6495ED",
          "#8FBC8F",
          "#D2691E",
          "#800080",
          "#4682B4",
          "#A0522D",
          "#008080",
          "#BC8F8F",
          "#2F4F4F",
          "#556B2F"
        ]

        color = Enum.at(colors, rem(index, length(colors)))

        """
        style #{node_id} fill:#{color},color:#fff,stroke:#333,stroke-width:2px
        """
      end)
      |> Enum.join("\n")

    """
    graph LR
        classDef node fill:#f9f,stroke:#333,stroke-width:2px,color:#000;

        subgraph Cluster
        direction TB
        #{nodes_string}
        end
        #{connections_string}
        #{style_string}
    """
  end

  def generate_mermaid_sequence(node_data) do
    lifelines =
      node_data.nodes
      |> Enum.map(fn {node_id, _node_info} ->
        """
        participant #{node_id}
        """
      end)
      |> Enum.join("\n")

    messages_string =
      node_data.nodes
      |> Enum.flat_map(fn {source_id, node_info} ->
        messages = Map.get(node_info, :messages, [])

        messages
        |> Enum.with_index()
        |> Enum.map(fn {msg, index} ->
          """
          #{source_id}->>CentralAuth: M#{index + 1}: #{msg}
          activate CentralAuth
          deactivate CentralAuth
          """

          # You may want to infer target from message itself
        end)
      end)
      |> Enum.join("\n")

    """
    sequenceDiagram
    title Message Flow

    #{lifelines}
    participant CentralAuth

    #{messages_string}
    """
  end
end
