import './lib/styles/global.css';
import { mount } from 'svelte';
import App from './App.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('корневой узел #app не найден');

export default mount(App, { target });
