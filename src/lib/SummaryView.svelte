<script lang="ts">
	let { text }: { text: string } = $props();
	// Render text nodes, never model-generated HTML.
	function parts(line: string) { return line.split(/(\*\*[^*]+\*\*)/g); }
</script>
<div class="summary-prose">
	{#each text.split('\n') as line}
		{#if /^#{1,6}\s/.test(line)}<h3>{line.replace(/^#{1,6}\s+/, '')}</h3>
		{:else if /^\s*[-*]\s/.test(line)}<div class="bullet"><span>•</span><p>{#each parts(line.replace(/^\s*[-*]\s+/, '')) as part}{#if part.startsWith('**')}<strong>{part.slice(2,-2)}</strong>{:else}{part}{/if}{/each}</p></div>
		{:else if line.trim()}<p>{#each parts(line) as part}{#if part.startsWith('**')}<strong>{part.slice(2,-2)}</strong>{:else}{part}{/if}{/each}</p>{/if}
	{/each}
</div>
<style>
.summary-prose{font:17px/1.6 Georgia,serif;color:inherit}.summary-prose h3{font:600 19px/1.4 Georgia,serif;margin:24px 0 10px}.summary-prose p{margin:8px 0;white-space:pre-wrap;overflow-wrap:anywhere}.bullet{display:flex;gap:12px;padding-left:8px}.bullet>span{padding-top:8px}
</style>
