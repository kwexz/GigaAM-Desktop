<script lang="ts">
	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href="/app-icon.png" />
</svelte:head>

{@render children()}
