defmodule PhoenixClientExTest do
  use ExUnit.Case
  doctest PhoenixClientEx

  test "greets the world" do
    assert PhoenixClientEx.hello() == :world
  end
end
