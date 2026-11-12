<script lang="ts">
	import type { Snippet } from 'svelte';
	let { label, title, variant = 'footer', children, trigger, items, value, onchange, disabled = false }: { label: string; title?: string; variant?: 'header' | 'footer' | 'primary' | 'select'; children?: Snippet; trigger?: Snippet; items?: { value: string; label: string }[]; value?: string; onchange?: (value: string) => void; disabled?: boolean } = $props();
</script>

<details class:header={variant === 'header'} class:primary={variant === 'primary'} class:select={variant === 'select'}>
	<summary aria-label={label} aria-disabled={disabled} {title} onclick={(event) => { if (disabled) event.preventDefault(); }}>{#if trigger}{@render trigger()}{:else}<span class="selected-label">{items?.find((item) => item.value === value)?.label ?? value ?? label}</span><svg class="chevron" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>{/if}</summary>
	<div class="menu" onclick={(event) => { if ((event.target as Element).closest('button,a')) event.currentTarget.closest('details')!.open = false; }} onkeydown={() => {}} role="presentation">{#if items}{#each items as item}<button type="button" aria-current={item.value === value ? 'true' : undefined} onclick={() => onchange?.(item.value)}>{item.label}</button>{/each}{:else if children}{@render children()}{/if}</div>
</details>

<style>
	details { position:relative; }
	summary { list-style:none; cursor:pointer; display:inline-flex; align-items:center; justify-content:center; gap:7px; height:34px; box-sizing:border-box; padding:7px 12px; border:1px solid #d5d7d0; border-radius:8px; background:#fff; color:#343832; font-family:inherit; font-size:13px; font-weight:600; line-height:18px; }
	summary::-webkit-details-marker { display:none; }
	.header summary { width:36px; padding:0; font-size:24px; }
	.primary summary { background:#28705a; color:#fff; border-color:#28705a; }
	.select { width:100%; min-width:0; max-width:100%; }
	.select summary { width:100%; justify-content:space-between; font-weight:400; overflow:hidden; }
	.selected-label { min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; text-align:left; }
	.chevron { flex:none; }
	.header summary { line-height:1; }
	.select summary[aria-disabled="true"] { opacity:.55; cursor:not-allowed; }
	.select .menu { top:calc(100% + 4px); bottom:auto; right:0; min-width:100%; box-sizing:border-box; max-height:240px; overflow:auto; }
	.menu { position:absolute; z-index:50; bottom:calc(100% + 6px); left:0; min-width:200px; display:flex; flex-direction:column; gap:4px; padding:6px; background:#fff; border:1px solid #d5d7d0; border-radius:10px; box-shadow:0 8px 28px #0002; }
	.header .menu { top:calc(100% + 6px); bottom:auto; left:auto; right:0; min-width:160px; }
	.menu :global(button) { display:block; width:100%; min-height:34px; height:auto; padding:8px 10px; border:0 !important; border-radius:6px; background:transparent; color:#343832; text-align:left; font-family:inherit; font-size:13px; font-weight:600; line-height:18px; cursor:pointer; white-space:nowrap; }
	.menu :global(button:hover), .menu :global(button:focus-visible) { background:#edf1eb; }
	.menu :global(button:focus-visible) { outline:2px solid #28705a; outline-offset:-2px; }
	:global(body[data-theme="dark"]) summary, :global(body[data-theme="dark"]) .menu { background:#262926; color:#eef1eb; border-color:#59645a; }
	:global(body[data-theme="dark"]) .primary summary { background:#28705a; color:white; border-color:#28705a; }
	:global(body[data-theme="dark"]) .menu :global(button) { color:#eef1eb; }
	:global(body[data-theme="dark"]) .menu :global(button:hover), :global(body[data-theme="dark"]) .menu :global(button:focus-visible) { background:#3a433b; }
</style>
