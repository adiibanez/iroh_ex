defmodule IrohEx.MixProject do
  use Mix.Project

  @version "0.0.15-alpha"
  @source_url "https://github.com/adiibanez/iroh_ex"
  @dev? String.ends_with?(@version, "-dev")

  def project do
    [
      app: :iroh_ex,
      version: @version,
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  # Run "mix help compile.app" to learn about applications.
  def application do
    [
      extra_applications: [:logger]
    ]
  end

  # Run "mix help deps" to learn about dependencies.
  defp deps do
    [
      {:rustler, "~> 0.36.1"},
      {:rustler_precompiled, "~> 0.7"}
      # {:dep_from_hexpm, "~> 0.3.0"},
      # {:dep_from_git, git: "https://github.com/elixir-lang/my_dep.git", tag: "0.1.0"}
    ]
  end
end
