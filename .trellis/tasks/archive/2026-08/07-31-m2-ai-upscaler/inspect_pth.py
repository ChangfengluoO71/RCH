"""检查 pth 文件结构"""
import torch
state = torch.load(r'c:\Users\cfl\Desktop\RCH\app\windows\ai\models\realesr-animevideov3.pth', map_location='cpu', weights_only=True)
print('Type:', type(state))
print('Keys:', list(state.keys()) if isinstance(state, dict) else 'not a dict')
if isinstance(state, dict):
    for k in list(state.keys())[:10]:
        v = state[k]
        if isinstance(v, dict):
            print(f'  {k}: dict with keys {list(v.keys())[:10]}')
        elif hasattr(v, 'shape'):
            print(f'  {k}: shape {v.shape}')
        else:
            print(f'  {k}: {type(v).__name__}')
