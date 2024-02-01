import React, { Suspense } from "react";
import { createRootRoute, Outlet } from "@tanstack/react-router";

import { Toaster } from "@/components/ui/toaster";

const TanStackRouterDevtools =
  process.env.NODE_ENV === "production"
    ? () => null
    : React.lazy(() =>
        import("@tanstack/router-devtools").then((res) => ({
          default: res.TanStackRouterDevtools,
        })),
      );

export const Route = createRootRoute({
  component: () => (
    <>
      <Outlet />
      <Suspense>
        <TanStackRouterDevtools />
      </Suspense>
      <Toaster />
    </>
  ),
});
