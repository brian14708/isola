import { createClient, type Client } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import type { DescService } from "@bufbuild/protobuf";
import { useMemo } from "react";

const transport = createGrpcWebTransport({
  baseUrl: "/",
  useBinaryFormat: true,
  interceptors: [],
});

const clientMap: Record<string, Client<DescService>> = {};

export function useRpcClient<T extends DescService>(service: T): Client<T> {
  return useMemo<Client<T>>(() => {
    const name = service.typeName;
    if (clientMap[name]) {
      return clientMap[name] as Client<T>;
    }

    const newClient = createClient(service, transport);
    // Safe: This is a module-level cache that needs to be populated during memoization
    clientMap[name] = newClient; // eslint-disable-line react-hooks/immutability
    return newClient;
  }, [service]);
}
