"""打印 pth 文件中所有权重的 key 和 shape，用于推断正确的模型结构"""
import torch

PTH_PATH = r'c:\Users\cfl\Desktop\RCH\app\windows\ai\models\realesr-animevideov3.pth'
state = torch.load(PTH_PATH, map_location='cpu', weights_only=True)
params = state.get('params', state)

for k, v in sorted(params.items()):
    print(f"  {k}: {tuple(v.shape)}")
