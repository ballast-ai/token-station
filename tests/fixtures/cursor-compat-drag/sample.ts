type User = { id: number; name: string };

export function display(user: User): string {
  return `${user.id}: ${user.name}`;
}

export const sample: User = { id: 1, name: "Ada" };
