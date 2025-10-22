# Generating Polars code with LLMs

Large Language Models (LLMs) can sometimes return pandas code or invalid Polars code in their
output. This guide presents approaches that help LLMs generate valid Polars code more consistently.

These approaches have been developed by the Polars community through testing model responses to
various inputs. If you find additional effective approaches for generating Polars code from LLMs,
please raise a [pull request](https://github.com/pola-rs/polars/pulls).

## Polars MCP server

The new remote Model Context Protocol (MCP) server for Polars provides access to the latest version
of the official Polars and Polars Cloud documentation. The server enables LLMs to query the user
guide and API references directly, making it easier to get more accurate answers about DataFrame
operations, expressions, lazy evaluation, and cloud deployments. The MCP server delivers up-to-date
documentation to help you rewrite existing queries from other libraries to Polars or work with
Polars more efficiently.

```json
{
  "mcpServers": {
    "ask_polars": {
      "command": "npx",
      "args": ["mcp-remote", "https://mcp.polars.workers.dev/sse"]
    }
  }
}
```

If you run into an issue or are missing a feature, please
[open an issue](https://github.com/pola-rs/polars/issues) on the public issue tracker. We plan to
expand the capabilities over time.

!!! note "MCP server installation"

    Please refer to the documentation of your preferred client to connect to the MCP server.

## System prompt

Many LLMs allow you to provide a system prompt that is included with every individual prompt you
send to the model. In the system prompt, you can specify your preferred defaults, such as "Use
Polars as the default dataframe library". Including such a system prompt typically leads to models
consistently generating Polars code rather than Pandas code.

You can set this system prompt in the settings menu of both web-based LLMs like ChatGPT and
IDE-based LLMs like Cursor. Refer to each application's documentation for specific instructions.

## Enable web search

Some LLMs can search the web to access information beyond their pre-training data. Enabling web
search allows an LLM to reference up-to-date Polars documentation for the current API.

Some IDE-based LLMs can index the Polars API documentation and reference this when generating code.
For example, in Cursor you can add Polars as a custom docs source and instruct the agent to
reference the Polars documentation in a prompt.

However, web search does not yet guarantee that valid code will be produced. If a model is confident
in a result based on its pre-training data, it may not incorporate web search results in its output.

The Polars API pages also have AI-enabled search to help you find the information you need more
easily.

## Provide examples

You can guide LLMs to use correct syntax by including relevant examples in your prompt.

For instance, this basic query:

```python
df = pl.DataFrame({
    "id": ["a", "b", "a", "b", "c"],
    "score": [1, 2, 1, 3, 3],
    "year": [2020, 2020, 2021, 2021, 2021],
})
# Compute average of score by id
```

Often results in outdated `groupby` syntax instead of the correct `group_by`.

However, including a simple example from the Polars `group_by` documentation (preferably with web
search enabled) like this:

```python
df = pl.DataFrame({
    "id": ["a", "b", "a", "b", "c"],
    "score": [1, 2, 1, 3, 3],
    "year": [2020, 2020, 2021, 2021, 2021],
})
# Compute average of score by id
# Examples of Polars code:

# df.group_by("a").agg(pl.col("b").mean())
```

Produces valid outputs more often. This approach has been validated across several leading models.

The combination of web search and examples is more effective than either independently. Model
outputs indicate that when an example contradicts the model's pre-trained expectations, it seems
more likely to trigger a web search for verification.

Additionally, explicit instructions like "use `group_by` instead of `groupby`" can be effective in
guiding the model to use correct syntax.

Common examples such as `df.group_by("a").agg(pl.col("b").mean())` can also be added the system
prompt for more consistency.
