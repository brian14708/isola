import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import Icons from "unplugin-icons/vite";

export default defineConfig({
	plugins: [
		sveltekit(),
		tailwindcss(),
		Icons({
			compiler: "svelte",
		}),
	],
	server: {
		proxy: {
			"/v1/": {
				target: "http://localhost:3000",
			},
			"/promptkit.": {
				target: "http://localhost:3000",
			},
		},
	},
});
