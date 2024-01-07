import { NextUIProvider, Switch } from '@nextui-org/react';
import { Outlet, useNavigate } from 'react-router-dom';
import { FiSun, FiMoon } from 'react-icons/fi';
import { useTheme } from '@/hooks/theme';

export default function Layout() {
	const { theme, toggleTheme } = useTheme();
	const navigate = useNavigate();

	return (
		<NextUIProvider navigate={navigate}>
			<div className="flex">
				<Switch
					defaultSelected={theme.current === 'light'}
					color="default"
					thumbIcon={({ isSelected, className }) =>
						isSelected ? <FiSun className={className} /> : <FiMoon className={className} />
					}
					onValueChange={(e) => {
						toggleTheme({ mode: e ? 'light' : 'dark' });
					}}
				/>
				<Outlet />
			</div>
		</NextUIProvider>
	);
}
