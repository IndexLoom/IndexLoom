import { useEffect, useState } from "react";

export function NetworkStatus() {
  const [online, setOnline] = useState(() => navigator.onLine);
  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);
  return online ? null : (
    <p className="network-offline" role="status">
      You are offline. Your interface is available, but account and indexer operations need a connection.
    </p>
  );
}
