<script lang="ts">
	import type { Snippet } from 'svelte';
	let { label, title, variant = 'footer', children, trigger }: { label: string; title?: string; variant?: 'header' | 'footer' | 'primary'; children: Snippet; trigger: Snippet } = $props();
</script>

<details class:header={variant === 'header'} class:primary={variant === 'primary'}>
	<summary aria-label={label} {title}>{@render trigger()}</summary>
	<div class="menu" onclick={(event) => { if ((event.target as Element).closest('button,a')) event.currentTarget.closest('details')!.open = false; }} onkeydown={() => {}} role="presentation">{@render children()}</div>
</details>

<style>
	details { position:relative; }
	summary { list-style:none; cursor:pointer; display:inline-flex; align-items:center; justify-content:center; gap:7px; height:34px; box-sizing:border-box; padding:7px 12px; border:1px solid #d5d7d0; border-radius:8px; background:#fff; color:#343832; font:600 13px/18px inherit; }
	summary::-webkit-details-marker { display:none; }
	.header summary { width:36px; padding:0; font-size:24px; }
	.primary summary { background:#28705a; color:#fff; border-color:#28705a; }
	.menu { position:absolute; z-index:50; bottom:calc(100% + 6px); left:0; min-width:200px; display:flex; flex-direction:column; gap:4px; padding:6px; background:#fff; border:1px solid #d5d7d0; border-radius:10px; box-shadow:0 8px 28px #0002; }
	.header .menu { top:calc(100% + 6px); bottom:auto; left:auto; right:0; min-width:160px; }
	.menu :global(button) { display:block; width:100%; min-height:34px; padding:8px 10px; border:0; border-radius:6px; background:transparent; color:#343832; text-align:left; font:600 13px/18px inherit; cursor:pointer; white-space:nowrap; }
	.menu :global(button:hover), .menu :global(button:focus-visible) { background:#edf1eb; }
	:global(body[data-theme="dark"]) summary, :global(body[data-theme="dark"]) .menu { background:#262926; color:#eef1eb; border-color:#59645a; }
	:global(body[data-theme="dark"]) .menu :global(button) { color:#eef1eb; }
	:global(body[data-theme="dark"]) .menu :global(button:hover), :global(body[data-theme="dark"]) .menu :global(button:focus-visible) { background:#3a433b; }
</style>
