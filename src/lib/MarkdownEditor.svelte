<script lang="ts">
	import { onMount } from 'svelte';
	import '@toast-ui/editor/dist/toastui-editor.css';
	import '@toast-ui/editor/dist/theme/toastui-editor-dark.css';
	let { value = $bindable('') }: { value: string } = $props();
	let root: HTMLDivElement;
	let unavailable = $state(false);
	onMount(() => {
		let editor: import('@toast-ui/editor').default | undefined;
		let disposed = false;
		void import('@toast-ui/editor').then(({ default: Editor }) => {
			if (disposed) return;
			const dark = document.body.dataset.theme === 'dark' || (!document.body.dataset.theme && window.matchMedia('(prefers-color-scheme: dark)').matches);
			editor = new Editor({ el: root, initialValue: value, initialEditType: 'wysiwyg', previewStyle: 'tab', height: '100%', usageStatistics: false, hideModeSwitch: false, theme: dark ? 'dark' : undefined, toolbarItems: [['heading', 'bold', 'italic'], ['ul', 'ol'], ['quote', 'code']] });
			editor.on('change', () => { value = editor!.getMarkdown(); });
		}).catch(() => { if (!disposed) unavailable = true; });
		return () => { disposed = true; editor?.destroy(); };
	});
</script>
<div class="markdown-editor" bind:this={root}></div>
{#if unavailable}<textarea aria-label="Markdown" bind:value rows="16"></textarea>{/if}
<style>
	.markdown-editor { flex:1; min-height:0; height:100%; }
	:global(.markdown-editor .toastui-editor-defaultUI) { min-height:0; }
	:global(.toastui-editor-md-mode .toastui-editor-ww-container),
	:global(.toastui-editor-ww-mode .toastui-editor-md-container) { display:none !important; }
	:global(.toastui-editor-md-mode .toastui-editor-md-container),
	:global(.toastui-editor-ww-mode .toastui-editor-ww-container) { display:block; }
</style>
