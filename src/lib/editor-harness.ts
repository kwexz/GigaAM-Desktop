import { mount } from 'svelte';
import MarkdownEditor from './MarkdownEditor.svelte';
import Dropdown from './Dropdown.svelte';

export function mountEditor(target: HTMLElement) {
	return mount(MarkdownEditor, { target, props: { value: '# Heading\n\n**Bold text**' } });
}

export function mountDropdown(target: HTMLElement) {
	const value = 'Logitech Webcam with a very long microphone name that exceeds the available width';
	return mount(Dropdown, { target, props: { variant: 'select', label: 'Microphone', value, items: [{ value, label: value }] } });
}
