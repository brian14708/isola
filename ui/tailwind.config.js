import { nextui } from '@nextui-org/react';
import defaultTheme from 'tailwindcss/defaultTheme';

/** @type {import('tailwindcss').Config} */
export default {
	content: [
		'./index.html',
		'./src/**/*.{js,ts,jsx,tsx}',
		'./node_modules/@nextui-org/theme/dist/**/*.{js,ts,jsx,tsx}'
	],
	theme: {
		extend: {
			fontFamily: {
				sans: ['"Inter Variable"', ...defaultTheme.fontFamily.sans]
			}
		}
	},
	darkMode: 'class',
	plugins: [nextui()]
};
