<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let name = $state("");
  let greeting = $state("");

  async function greet() {
    greeting = await invoke<string>("greet", { name: name || "world" });
  }
</script>

<input bind:value={name} placeholder="Enter a name" />
<button onclick={greet}>Greet</button>
<p>{greeting}</p>
