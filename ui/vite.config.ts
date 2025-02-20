import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
	plugins: [sveltekit(), tailwindcss()],
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
