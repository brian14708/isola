import { useEffect, useState } from 'react';
import { Theme, ThemeContext } from '@/hooks/theme';

export default function ThemeProvider({ children }: { children: React.ReactNode }) {
	const [theme, setTheme] = useState<Theme>({
		mode: 'auto',
		current: 'light'
	});
	const toggleTheme = (theme?: Omit<Theme, 'current'>) => {
		if (theme) {
			let dark = theme.mode === 'dark';
			if (theme.mode === 'auto') {
				dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
			}
			const t = {
				mode: theme.mode,
				current: dark ? 'dark' : 'light'
			} satisfies Theme;
			setTheme(t);
			localStorage.setItem('theme', JSON.stringify(t));
			document.documentElement.classList.toggle('dark', dark);
		} else {
			let dark = false;
			setTheme((curr) => {
				dark = curr.current === 'light';
				return {
					mode: dark ? 'dark' : 'light',
					current: dark ? 'dark' : 'light'
				};
			});
			localStorage.setItem(
				'theme',
				JSON.stringify({
					mode: dark ? 'dark' : 'light',
					current: dark ? 'dark' : 'light'
				})
			);
			document.documentElement.classList.toggle('dark', dark);
		}
	};
	useEffect(() => {
		const store = localStorage.getItem('theme');
		if (store) {
			toggleTheme(JSON.parse(store));
		} else {
			toggleTheme({ mode: 'auto' });
		}
	}, []);
	return <ThemeContext.Provider value={{ theme, toggleTheme }}>{children}</ThemeContext.Provider>;
}
