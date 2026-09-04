import {
  BadRequestException,
  Body,
  Controller,
  Delete,
  Get,
  Inject,
  OnModuleInit,
  Param,
  Put,
  UseGuards,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { Admin, AuthGuard } from 'src/auth/auth.guard';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { PutRegionDto, REGION_TOKEN, RegionDto } from './dto/region.dto';

/**
 * The regions the control plane knows. Read by anyone signed in (the
 * console offers them as homes); written by the instance admin, who is
 * the one standing up regions.
 */
@UseGuards(AuthGuard)
@ApiTags('regions')
@ApiBearerAuth()
@Controller('regions')
export class RegionsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(@Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Every registered region. Empty on a single-region deployment.
   */
  @Get()
  async listRegions(): Promise<RegionDto[]> {
    const listed = await lastValueFrom(
      this.scriptService.listRegions({}).pipe(toHttpException()),
    );
    return (listed.regions ?? []).map(RegionDto.fromProto);
  }

  /**
   * Registers or updates a region: its data-plane address and bucket.
   */
  @Put(':name')
  @Admin()
  @ApiParam({ name: 'name', schema: { type: 'string' }, type: 'string' })
  async putRegion(
    @Param('name') name: string,
    @Body() body: PutRegionDto,
  ): Promise<RegionDto> {
    if (!REGION_TOKEN.test(name)) {
      throw new BadRequestException(
        `'${name}' is not a region: 1 to 16 of a-z, 0-9 and '-', not starting with '-'.`,
      );
    }
    const stored = await lastValueFrom(
      this.scriptService
        .putRegion({
          name,
          dataPlaneAddr: body.dataPlaneAddr,
          bucket: body.bucket,
          placementAddr: body.placementAddr ?? '',
          s3Endpoint: body.s3Endpoint ?? '',
          s3AccessKey: body.s3AccessKey ?? '',
          s3SecretKey: body.s3SecretKey ?? '',
        })
        .pipe(toHttpException()),
    );
    return RegionDto.fromProto(stored);
  }

  /**
   * Forgets a region. Refused while any project calls it home.
   */
  @Delete(':name')
  @Admin()
  @ApiParam({ name: 'name', schema: { type: 'string' }, type: 'string' })
  async deleteRegion(@Param('name') name: string): Promise<void> {
    await lastValueFrom(
      this.scriptService.deleteRegion({ name }).pipe(toHttpException()),
    );
  }
}
