import { mount } from 'svelte';
import './styles.css';
import App from './App.svelte';

// No browser context menu or text selection outside inputs: this is an app window.
window.addEventListener('contextmenu', (e) => {
  const t = e.target as HTMLElement;
  if (!(t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t.closest('.selectable'))) e.preventDefault();
});

export default mount(App, { target: document.getElementById('app')! });
