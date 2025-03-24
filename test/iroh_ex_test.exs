defmodule IrohExTest do
  use ExUnit.Case
  doctest IrohEx
  alias IrohEx.Native

  test "greets the world" do
    assert IrohEx.hello() == :world
  end

  test "create iroh endpoint" do
    mother_node_ref = Native.create_node(self())

    ticket = Native.create_ticket(mother_node_ref)

    IO.inspect(ticket, label: "Node1 ticket")

    count = 20

    Task.async(fn -> Native.connect_node(mother_node_ref, ticket) end)

    nodes =
      Enum.map(1..count, fn _ ->
        Native.create_node(self())
      end)

    tasks =
      Enum.map(nodes, fn node ->
        Task.async(fn -> Native.connect_node(node, ticket) end)
      end)

    # Enum.each(tasks, &Task.await/1)

    Process.sleep(2000)

    Enum.each(1..20, fn x ->
      node = Enum.random(nodes)

      node
      |> Native.send_message("Elixir: Message #{inspect(x)}")
    end)

    Process.sleep(5000)

    assert IrohEx.hello() == :world
  end
end
